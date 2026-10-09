use clap::{Parser, Subcommand};
use macbot_gateway::{run, GatewayConfig};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Parser)]
#[command(name = "macbotd", version, about = "Mac Bot host daemon")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long)]
    mock: bool,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long, env = "MACBOT_HOME")]
    home: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Status,
    Passwd {
        #[arg(long)]
        password: Option<String>,
    },
    Logs {
        #[arg(short)]
        follow: bool,
    },
    Restart,
    Update,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let home = args
        .home
        .clone()
        .or_else(|| std::env::var_os("MACBOT_HOME").map(PathBuf::from))
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("MacBot")
        });
    if let Some(command) = args.command {
        return run_command(command, home).await;
    }
    let port = args.port.unwrap_or(if args.mock { 7789 } else { 7788 });
    run(GatewayConfig {
        bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
        home,
        password: args.password,
        mock: args.mock,
        ..Default::default()
    })
    .await
}

async fn run_command(
    command: Command,
    home: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        Command::Status => {
            let health = "http://127.0.0.1:7788/api/v1/health".to_string();
            let output = std::process::Command::new("curl")
                .args(["-fsS", &health])
                .output();
            match output {
                Ok(out) if out.status.success() => {
                    print!("{}", String::from_utf8_lossy(&out.stdout))
                }
                _ => println!("macbotd is not reachable"),
            }
        }
        Command::Passwd { password } => {
            let password = password
                .or_else(|| std::env::var("MACBOT_PASSWORD").ok())
                .ok_or("provide --password or MACBOT_PASSWORD")?;
            let gateway = macbot_gateway::Gateway::new(GatewayConfig {
                home,
                ..Default::default()
            });
            gateway.set_password(&password).await?;
            println!("password updated");
        }
        Command::Logs { follow } => {
            let path = home.join("data/macbot.log");
            if follow {
                let _ = std::process::Command::new("tail")
                    .args(["-f", path.to_string_lossy().as_ref()])
                    .status();
            } else if let Ok(s) = std::fs::read_to_string(path) {
                print!("{s}");
            }
        }
        Command::Restart => {
            let uid = std::process::Command::new("id")
                .arg("-u")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .filter(|uid| !uid.is_empty())
                .ok_or("cannot determine current uid")?;
            let _ = std::process::Command::new("launchctl")
                .args(["kickstart", "-k", &format!("gui/{uid}/com.macbot.server")])
                .status();
        }
        Command::Update => {
            let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packaging/update.sh");
            let status = std::process::Command::new("sh").arg(script).status()?;
            if !status.success() {
                return Err("update failed".into());
            }
        }
    }
    Ok(())
}
