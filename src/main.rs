use anyhow::Result;
use cfvirtual::{
    cf::CfClient,
    cli::{Cli, Command},
    config::Paths,
};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Start(args) = cli.command {
        let handle = args
            .handle
            .ok_or_else(|| anyhow::anyhow!("provide --handle"))?;
        let paths = Paths::discover()?;
        let api = CfClient::new();
        let user = api.user_info(&handle).await?;
        let history = api.history(&handle).await?;
        let problems = api
            .problemset(&paths.data.join("problemset.json"), args.refresh)
            .await?;
        println!(
            "Profile: {} ({}); submissions: {}; cached problems: {}",
            user.handle,
            user.rating,
            history.len(),
            problems.len()
        );
    } else {
        eprintln!("This milestone provides --help and start profile/cache inspection.");
    }
    Ok(())
}
