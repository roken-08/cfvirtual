use anyhow::Result;
use cfvirtual::{
    cf::CfClient,
    cli::{Cli, Command},
    config::{Config, Paths},
    domain::scoring,
    service::{self, ContestOptions},
};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Start(args) = cli.command {
        let paths = Paths::discover()?;
        let config = Config::load(&paths.config)?;
        let handle = args.handle.unwrap_or(config.handle);
        let api = CfClient::new();
        let options = ContestOptions {
            handle,
            problems: args
                .problems
                .map(|n| n as usize)
                .unwrap_or(config.default_problems),
            duration_sec: args.duration.unwrap_or(config.default_duration) * 60,
            rating: args.rating,
            tags: args.tags,
            bots: args.bots.map(|n| n as usize).unwrap_or(config.default_bots),
            seed: args.seed.unwrap_or(42),
            blind: args.blind,
            refresh: args.refresh,
        };
        let mut contest = service::prepare(&options, &api, &paths.data, "solo").await?;
        for problem in &contest.problems {
            println!(
                "{}: {} {}",
                problem.slot,
                problem.problem.name,
                problem.problem.url()
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        contest.started_at = chrono::Utc::now().timestamp();
        let mut submissions = Vec::new();
        loop {
            let now = chrono::Utc::now().timestamp();
            println!(
                "{} seconds remaining: {:?}",
                (contest.deadline() - now).max(0),
                scoring::score(&contest, &submissions, now)
            );
            if now >= contest.deadline() {
                break;
            }
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {}
            }
            if let Ok(recent) = api.recent(&contest.host_handle).await {
                submissions.extend(recent);
            }
        }
    } else {
        eprintln!("Use start to run the solo text-mode milestone.");
    }
    Ok(())
}
