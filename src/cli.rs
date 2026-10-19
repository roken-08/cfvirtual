use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Read-only Codeforces virtual contests in your terminal",
    long_about = "Run solo contests or trust-based LAN rooms. Submit manually in your browser; only public Codeforces APIs are used."
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Emit only JSON on stdout (headless for start/demo)"
    )]
    pub json: bool,
    #[arg(long, global = true, help = "Use ASCII borders and status symbols")]
    pub ascii: bool,
    #[arg(long, global = true, value_parser = ["default", "colorblind"])]
    pub theme: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Assemble and run a solo contest; --json runs headlessly until the deadline.
    Start(ContestArgs),
    /// Create a multiplayer room.
    Room {
        #[command(subcommand)]
        command: RoomCommand,
    },
    /// Join a room. No Codeforces credentials are needed.
    Join(JoinArgs),
    /// Run a headless WebSocket room server.
    Serve {
        #[arg(long, default_value = "0.0.0.0:7331")]
        listen: std::net::SocketAddr,
        #[arg(long)]
        public_url: Option<String>,
    },
    /// List saved contests or export a reproducible Markdown review.
    History {
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
        #[arg(long, help = "Export this contest ID to a Markdown file")]
        export: Option<String>,
        #[arg(long)]
        handle: Option<String>,
    },
    /// List review problems; manually mark a practice outcome.
    Review {
        #[arg(long)]
        due: bool,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
        #[arg(long, conflicts_with = "skip")]
        solve: Option<String>,
        #[arg(long)]
        skip: Option<String>,
    },
    /// Show saved-contest totals and per-tag statistics.
    Stats,
    /// Read or atomically update local configuration.
    Config {
        #[arg(value_enum, default_value_t = ConfigAction::Get)]
        action: ConfigAction,
        key: Option<String>,
        value: Option<String>,
    },
    /// Offline six-problem, three-bot demo (90 seconds; accelerated without a TTY).
    Demo {
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum RoomCommand {
    Create {
        #[command(flatten)]
        contest: ContestArgs,
        #[arg(long, default_value_t = 7331)]
        port: u16,
        #[arg(long, default_value = "0.0.0.0")]
        listen: std::net::IpAddr,
        #[arg(
            long,
            help = "Optional trust-based room gate; not account authentication"
        )]
        password: Option<String>,
        #[arg(long)]
        public_url: Option<String>,
        #[arg(
            long,
            help = "Create on an existing headless server instead of embedding one"
        )]
        server: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct ContestArgs {
    #[arg(long)]
    pub handle: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=26))]
    pub problems: Option<u32>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1440), help = "Contest duration in minutes")]
    pub duration: Option<u32>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(800..=3500))]
    pub rating: Option<u32>,
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=16))]
    pub bots: Option<u32>,
    #[arg(long)]
    pub seed: Option<u64>,
    #[arg(long, conflicts_with = "reveal")]
    pub blind: bool,
    #[arg(long, help = "Allow ratings/tags during the contest")]
    pub reveal: bool,
    #[arg(long, help = "Refresh the 24-hour public problemset cache")]
    pub refresh: bool,
}

#[derive(Debug, Args)]
pub struct JoinArgs {
    #[arg(long)]
    pub server: Option<String>,
    #[arg(long)]
    pub code: String,
    #[arg(long)]
    pub handle: Option<String>,
    #[arg(long)]
    pub spectate: bool,
    #[arg(long, help = "Optional room gate (prompted if required)")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ConfigAction {
    Get,
    Set,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_tree_and_bounds_are_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        assert!(Cli::try_parse_from(["cfvirtual", "start", "--problems", "0"]).is_err());
        assert!(Cli::try_parse_from(["cfvirtual", "demo", "--json", "--ascii"]).is_ok());
        assert!(
            Cli::try_parse_from(["cfvirtual", "room", "create", "--reveal", "--bots", "16"])
                .is_ok()
        );
    }
}
