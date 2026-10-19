use anyhow::{bail, Context, Result};
use cfvirtual::{
    cf::{CfClient, CfError},
    cli::{Cli, Command, ConfigAction, ContestArgs, JoinArgs, RoomCommand},
    client::net::{Client, ClientMessage, ReconnectSession, ServerMessage},
    config::{self, Config, Paths},
    domain::{review, Contest, Standing},
    server,
    service::{self, ContestOptions},
    store::Store,
    tui,
};
use clap::Parser;
use futures_util::FutureExt;
use serde::Serialize;
use std::{
    io::{IsTerminal, Write},
    net::{IpAddr, SocketAddr},
    panic::AssertUnwindSafe,
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Usage(String);

fn usage(message: impl Into<String>) -> anyhow::Error {
    Usage(message.into()).into()
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::env::var("RUST_BACKTRACE").as_deref() == Ok("1") {
            original_hook(info);
        }
    }));
    match AssertUnwindSafe(run(cli)).catch_unwind().await {
        Ok(Ok(())) => std::process::ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("cfvirtual: {error:#}");
            let code = if error.downcast_ref::<Usage>().is_some() {
                2
            } else if let Some(cf) = error.downcast_ref::<CfError>() {
                if cf.is_invalid_handle() {
                    2
                } else {
                    eprintln!("Codeforces is unavailable. Retry later; cached solo practice may still work.");
                    3
                }
            } else {
                1
            };
            std::process::ExitCode::from(code)
        }
        Err(_) => {
            eprintln!("cfvirtual: an unexpected failure stopped the application. Local data has been retained; set RUST_BACKTRACE=1 for diagnostics.");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let paths = Paths::discover()?;
    let mut config = Config::load(&paths.config)?;
    let theme = cli.theme.as_deref().unwrap_or(&config.theme).to_string();
    match cli.command {
        Command::Start(args) => {
            if !cli.json {
                require_terminal()?;
            }
            let options = options(args, &mut config, &paths, false).await?;
            eprintln!(
                "Preparing contest from the public API/cache (requests are paced at 2 seconds)..."
            );
            let api = CfClient::new();
            let contest = service::prepare(&options, &api, &paths.data, "solo").await?;
            if cli.json {
                let (contest, standings) = service::run_headless(contest, api, &paths.db()).await?;
                json(&serde_json::json!({"contest":contest, "standings":standings}))?;
            } else {
                tui::run_solo(contest, api, paths.db(), &theme, cli.ascii, false).await?;
            }
        }
        Command::Demo { seed } => {
            if cli.json || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                let (contest, standings) = tui::run_demo_headless(seed, &paths.db()).await?;
                if cli.json {
                    json(&serde_json::json!({"contest":contest,"standings":standings}))?;
                } else {
                    println!(
                        "Offline demo complete: 6 problems, 3 BOT opponents, 90 simulated seconds."
                    );
                    print_standings(&standings);
                    println!(
                        "Saved contest {}. Use history to export a review.",
                        contest.id
                    );
                }
            } else {
                tui::run_solo(
                    tui::demo_contest(seed),
                    CfClient::new(),
                    paths.db(),
                    &theme,
                    cli.ascii,
                    true,
                )
                .await?;
            }
        }
        Command::Config { action, key, value } => match action {
            ConfigAction::Set => {
                let key = key.ok_or_else(|| usage("config set requires a key and value"))?;
                let value = value.ok_or_else(|| usage("config set requires a value"))?;
                config
                    .set(&key, &value, &paths.config)
                    .map_err(|e| usage(e.to_string()))?;
                if cli.json {
                    json(&serde_json::json!({"updated":key}))?;
                } else {
                    println!("Updated {key}");
                }
            }
            ConfigAction::Get => {
                if value.is_some() {
                    return Err(usage("config get accepts at most one key"));
                }
                let all = serde_json::to_value(&config)?;
                let output = if let Some(key) = key {
                    all.get(&key)
                        .cloned()
                        .ok_or_else(|| usage(format!("unknown config key: {key}")))?
                } else {
                    all
                };
                if cli.json {
                    json(&output)?;
                } else if let Some(text) = output.as_str() {
                    println!("{text}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
            }
        },
        Command::History {
            limit,
            export,
            handle,
        } => {
            let store = Store::open(&paths.db())?;
            if let Some(id) = export {
                let (contest, standings) = store
                    .load_contest(&id)?
                    .context("contest not found in local history")?;
                let handle = handle.unwrap_or_else(|| contest.host_handle.clone());
                if !standings
                    .iter()
                    .any(|s| !s.is_bot && s.name.eq_ignore_ascii_case(&handle))
                {
                    return Err(usage("handle is not a participant in this saved contest"));
                }
                let path = export_review(&contest, &standings, &handle)?;
                if cli.json {
                    json(&serde_json::json!({"path":path}))?;
                } else {
                    println!("Exported {path}");
                }
            } else {
                if handle.is_some() {
                    return Err(usage("--handle is only used with --export"));
                }
                let entries = store.history(limit as usize)?;
                if cli.json {
                    json(&entries)?;
                } else if entries.is_empty() {
                    println!("No saved contests. Try cfvirtual demo.");
                } else {
                    println!("Date (UTC)        Mode   Solved  Penalty(s)  Contest ID");
                    for c in entries {
                        println!(
                            "{}  {:6} {}/{}     {:10}  {}",
                            date(c.started_at),
                            c.mode,
                            c.solved,
                            c.problem_count,
                            c.penalty,
                            c.id
                        );
                    }
                }
            }
        }
        Command::Review {
            due,
            limit,
            solve,
            skip,
        } => {
            let mut store = Store::open(&paths.db())?;
            let now = chrono::Utc::now().timestamp();
            if let Some(key) = solve.as_ref().or(skip.as_ref()) {
                store.advance_review(key, solve.is_some(), now)?;
            }
            let entries = store.review(due, limit as usize, now)?;
            if cli.json {
                json(&entries)?;
            } else if entries.is_empty() {
                println!(
                    "No {}review problems. Queue unsolved problems with r on solo results.",
                    if due { "due " } else { "" }
                );
            } else {
                for p in entries {
                    println!(
                        "{}  {}  due {} ({}d)\n  {}",
                        p.problem_key,
                        p.name,
                        date(p.due_at),
                        p.interval_days,
                        p.url
                    );
                }
            }
        }
        Command::Stats => {
            let stats = Store::open(&paths.db())?.stats()?;
            if cli.json {
                json(&stats)?;
            } else {
                println!("Contests: {} | Solved: {}/{} ({:.1}%) | Average penalty: {:.0}s | UTC streak: {} days", stats.total_contests, stats.total_solved, stats.total_problems, stats.solve_rate*100.0, stats.average_penalty, stats.current_streak);
                for (tag, entry) in stats.per_tag {
                    println!(
                        "{tag:24} {}/{} ({:.1}%)",
                        entry.solved,
                        entry.problems,
                        entry.solve_rate * 100.0
                    );
                }
                if let Some(profile) = stats.history_profile {
                    println!(
                        "\nPublic-history weakness profile: {} (rating {}, fetched {} UTC)",
                        profile.handle,
                        profile.rating,
                        date(profile.updated_at)
                    );
                    for (tag, entry) in profile.tags {
                        println!(
                            "{tag:24} weakness {:.3} | solve rate {:.1}% | median solved rating {}",
                            entry.weakness,
                            entry.solve_rate * 100.0,
                            entry
                                .median_solved_rating
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "--".into())
                        );
                    }
                }
            }
        }
        Command::Room {
            command:
                RoomCommand::Create {
                    contest,
                    port,
                    listen,
                    password,
                    public_url,
                    server,
                },
        } => {
            if cli.json {
                return Err(usage(
                    "room create is interactive; use serve for headless hosting",
                ));
            }
            require_terminal()?;
            let options = options(contest, &mut config, &paths, true).await?;
            host_room(
                options, listen, port, password, public_url, server, &paths, &theme, cli.ascii,
            )
            .await?;
        }
        Command::Join(args) => {
            if cli.json {
                return Err(usage(
                    "join is interactive; --json is for read commands and headless solo/demo",
                ));
            }
            require_terminal()?;
            join(args, &mut config, &paths, &theme, cli.ascii).await?;
        }
        Command::Serve { listen, public_url } => {
            if cli.json {
                return Err(usage("serve has no JSON read output"));
            }
            if let Some(url) = public_url.as_ref() {
                config::validate_server(url).map_err(|e| usage(e.to_string()))?;
            }
            let listener = bind(listen).await?;
            warn_listen(listen.ip());
            eprintln!("Room server listening on {}. Create a room with room create --server {} --handle your_handle", listener.local_addr()?, public_url.unwrap_or_else(|| local_url(listener.local_addr().unwrap_or(listen))));
            axum::serve(
                listener,
                server::router(server::State::new(CfClient::new(), paths.data)),
            )
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        }
    }
    Ok(())
}

async fn options(
    args: ContestArgs,
    config: &mut Config,
    paths: &Paths,
    room: bool,
) -> Result<ContestOptions> {
    let handle = resolve_handle(args.handle, config, paths).await?;
    Ok(ContestOptions {
        handle,
        problems: args
            .problems
            .map(|n| n as usize)
            .unwrap_or(config.default_problems),
        duration_sec: args.duration.unwrap_or(config.default_duration) * 60,
        rating: args.rating,
        tags: args
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        bots: args
            .bots
            .map(|n| n as usize)
            .unwrap_or(if room { 0 } else { config.default_bots }),
        seed: args.seed.unwrap_or_else(rand::random),
        blind: !args.reveal && (room || args.blind),
        refresh: args.refresh,
    })
}

async fn resolve_handle(
    handle: Option<String>,
    config: &mut Config,
    paths: &Paths,
) -> Result<String> {
    if let Some(handle) =
        handle.or_else(|| (!config.handle.is_empty()).then(|| config.handle.clone()))
    {
        config::validate_handle(&handle).map_err(|e| usage(e.to_string()))?;
        return Ok(handle);
    }
    if !std::io::stdin().is_terminal() {
        return Err(usage("missing handle: pass --handle or config set handle"));
    }
    let handle = prompt("Public Codeforces handle: ")?;
    config::validate_handle(&handle).map_err(|e| usage(e.to_string()))?;
    eprintln!("Validating the public handle...");
    let user = CfClient::new().user_info(&handle).await?;
    config.set("handle", &user.handle, &paths.config)?;
    Ok(user.handle)
}

fn prompt(message: &str) -> Result<String> {
    eprint!("{message}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        bail!("input closed");
    }
    Ok(line.trim().to_string())
}

fn require_terminal() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(usage("interactive operation needs a terminal; use demo --json, start --json, or serve for headless operation"));
    }
    Ok(())
}

fn json(value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(std::io::stdout().lock(), value)?;
    println!();
    Ok(())
}

fn date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "invalid date".into())
}

fn print_standings(standings: &[Standing]) {
    for (rank, s) in standings.iter().enumerate() {
        println!(
            "{}  {}{}  {} solved  {}s penalty",
            rank + 1,
            s.name,
            if s.is_bot { " BOT" } else { "" },
            s.solved,
            s.penalty
        );
    }
}

fn export_review(contest: &Contest, standings: &[Standing], handle: &str) -> Result<String> {
    let filename = format!(
        "cfvirtual-review-{}.md",
        chrono::DateTime::from_timestamp(contest.started_at, 0)
            .context("invalid contest date")?
            .format("%Y-%m-%d-%H%M")
    );
    // Never silently replace an export from another contest in the same minute.
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&filename)
        .with_context(|| format!("create {filename}; move an existing export before retrying"))?;
    file.write_all(review::markdown(contest, standings, handle).as_bytes())?;
    file.sync_all()?;
    Ok(filename)
}

async fn bind(address: SocketAddr) -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(address).await.with_context(|| format!("cannot bind {address}; if the port is in use choose --port or --listen with another port"))
}

fn warn_listen(address: IpAddr) {
    if !address.is_loopback() {
        eprintln!("Warning: this listener exposes the room port to the LAN. Room codes are not authentication; use a password and trusted network.");
    }
}

fn local_url(address: SocketAddr) -> String {
    let ip = if address.ip().is_unspecified() {
        if address.is_ipv4() {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
    } else {
        address.ip()
    };
    format!("ws://{}", SocketAddr::new(ip, address.port()))
}

fn advertised_url(address: SocketAddr) -> String {
    if address.ip().is_unspecified() && address.is_ipv4() {
        // UDP connect only chooses a route; no datagram or private data is sent.
        let inferred = (|| -> std::io::Result<_> {
            let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })();
        if let Ok(local) = inferred {
            return format!("ws://{}", SocketAddr::new(local.ip(), address.port()));
        }
        eprintln!("Could not infer a LAN address. Replace 127.0.0.1 in the join command with this host's reachable IP.");
    }
    local_url(address)
}

struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// These arguments map one-to-one to the room-create command rather than being
// application-global state. Keep the embedded listener owned for the UI lifetime.
#[allow(clippy::too_many_arguments)]
async fn host_room(
    options: ContestOptions,
    listen: IpAddr,
    port: u16,
    password: Option<String>,
    public_url: Option<String>,
    remote: Option<String>,
    paths: &Paths,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    if let Some(url) = public_url.as_ref() {
        config::validate_server(url).map_err(|e| usage(e.to_string()))?;
    }
    let (client, display_url, _server_task) = if let Some(url) = remote {
        config::validate_server(&url).map_err(|e| usage(e.to_string()))?;
        let mut client = Client::connect(&url).await?;
        eprintln!("Preparing the room on the headless server...");
        client
            .send(ClientMessage::Create {
                options: options.clone(),
                password,
            })
            .await?;
        welcome(&mut client).await?;
        (client, url, None)
    } else {
        let listener = bind(SocketAddr::new(listen, port)).await?;
        let address = listener.local_addr()?;
        warn_listen(listen);
        let connect_url = local_url(address);
        let display_url = public_url.unwrap_or_else(|| advertised_url(address));
        let api = CfClient::new();
        let state = server::State::new(api.clone(), paths.data.clone());
        let router = server::router(state.clone());
        let task = ServerTask(tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                eprintln!("Room listener stopped: {error}");
            }
        }));
        eprintln!("Preparing the host profile and shared contest...");
        let contest = service::prepare(&options, &api, &paths.data, "room").await?;
        let created = state.create_room(contest, password).await?;
        let client = Client::resume(
            &connect_url,
            &ReconnectSession {
                room: created.code,
                participant_token: created.host_token,
            },
        )
        .await?;
        (client, display_url, Some(task))
    };
    let code = client
        .snapshot()
        .context("server did not provide a room snapshot")?
        .code
        .clone();
    eprintln!(
        "Room {code}: cfvirtual join --server {display_url} --code {code} --handle your_handle"
    );
    tui::run_room(
        client,
        display_url,
        options.handle,
        paths.db(),
        theme,
        ascii,
    )
    .await
}

async fn welcome(client: &mut Client) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            match client.recv().await? {
                Some(ServerMessage::Welcome {
                    participant_id,
                    state,
                    ..
                }) => {
                    return Ok(state
                        .participants
                        .iter()
                        .find(|p| p.participant_id == participant_id)
                        .map(|p| p.handle.clone())
                        .unwrap_or(state.host_handle));
                }
                Some(ServerMessage::Error { code, message }) => {
                    if ["INVALID_HANDLE", "INVALID_OPTIONS"].contains(&code.as_str()) {
                        return Err(usage(format!("{code}: {message}")));
                    }
                    bail!("{code}: {message}");
                }
                Some(_) => {}
                None => bail!("room server disconnected before welcome"),
            }
        }
    })
    .await
    .context("room welcome timed out; retry the connection")?
}

async fn join(
    args: JoinArgs,
    config: &mut Config,
    paths: &Paths,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    let url = args.server.unwrap_or_else(|| config.server.clone());
    config::validate_server(&url).map_err(|e| usage(e.to_string()))?;
    let handle = if args.spectate {
        String::new()
    } else {
        resolve_handle(args.handle, config, paths).await?
    };
    let mut password = args.password;
    let mut client = Client::connect(&url).await?;
    let message = |password: Option<String>| {
        if args.spectate {
            ClientMessage::Spectate {
                code: args.code.to_uppercase(),
                password,
            }
        } else {
            ClientMessage::Hello {
                code: args.code.to_uppercase(),
                handle: handle.clone(),
                password,
            }
        }
    };
    client.send(message(password.clone())).await?;
    let actual_handle = match welcome(&mut client).await {
        Ok(handle) => handle,
        Err(error) if password.is_none() && error.to_string().contains("BAD_PASSWORD") => {
            password = Some(prompt("Room password (visible input; soft gate only): ")?);
            client.send(message(password)).await?;
            welcome(&mut client).await?
        }
        Err(error) => return Err(error),
    };
    tui::run_room(client, url, actual_handle, paths.db(), theme, ascii).await
}
