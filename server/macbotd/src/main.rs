use clap::{Parser, Subcommand};
use macbot_gateway::{run, GatewayConfig};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

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
    Settings {
        #[arg(long)]
        host_name: Option<String>,
        #[arg(long)]
        port: Option<u16>,
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
    let saved = std::fs::read_to_string(home.join("data/settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let port = args
        .port
        .or_else(|| {
            saved
                .as_ref()
                .and_then(|value| value.get("port").and_then(|v| v.as_u64()).map(|v| v as u16))
        })
        .unwrap_or(if args.mock { 7789 } else { 7788 });
    let host_name = saved
        .as_ref()
        .and_then(|value| value.get("host_name").and_then(|v| v.as_str()))
        .unwrap_or("Mac Bot")
        .to_owned();
    run(GatewayConfig {
        bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
        home,
        password: args.password,
        mock: args.mock,
        host_name,
    })
    .await
}

async fn run_command(
    command: Command,
    home: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        Command::Status => {
            print!("{}", local_request(&home, "GET", "/__local/status", None)?);
        }
        Command::Passwd { password } => {
            let password = password
                .or_else(|| std::env::var("MACBOT_PASSWORD").ok())
                .ok_or("provide --password or MACBOT_PASSWORD")?;
            local_request(
                &home,
                "POST",
                "/__local/passwd",
                Some(json!({"password":password})),
            )?;
            println!("password updated");
        }
        Command::Settings { host_name, port } => {
            if host_name.is_none() && port.is_none() {
                return Err("provide --host-name and/or --port".into());
            }
            let mut patch = serde_json::Map::new();
            if let Some(host_name) = host_name {
                patch.insert("host_name".into(), json!(host_name));
            }
            if let Some(port) = port {
                patch.insert("port".into(), json!(port));
            }
            print!(
                "{}",
                local_request(&home, "POST", "/__local/settings", Some(json!(patch)))?
            );
        }
        Command::Logs { follow } => {
            if follow {
                let path = home.join("data/macbot.log");
                let _ = std::process::Command::new("tail")
                    .args(["-f", path.to_string_lossy().as_ref()])
                    .status();
            } else {
                print!("{}", local_request(&home, "GET", "/__local/logs", None)?);
            }
        }
        Command::Restart => {
            print!(
                "{}",
                local_request(&home, "POST", "/__local/restart", None)?
            );
        }
        Command::Update => {
            print!("{}", local_request(&home, "POST", "/__local/update", None)?);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn local_request(
    home: &Path,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use std::os::unix::net::UnixStream;
    let socket = home.join("data/macbotd.sock");
    let mut stream = UnixStream::connect(socket)?;
    let payload = body
        .map(|value| serde_json::to_vec(&value))
        .transpose()?
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let marker = b"\r\n\r\n";
    let offset = response
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or("invalid local daemon response")?;
    let body = &response[offset + marker.len()..];
    let text = String::from_utf8(body.to_vec())?;
    if !response.starts_with(b"HTTP/1.1 2") {
        return Err(format!("local daemon request failed: {text}").into());
    }
    Ok(text)
}

#[cfg(not(unix))]
fn local_request(
    _home: &Path,
    _method: &str,
    _path: &str,
    _body: Option<serde_json::Value>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Err("local CLI socket is only supported on Unix".into())
}
