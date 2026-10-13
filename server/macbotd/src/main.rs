use clap::{Parser, Subcommand};
use macbot_gateway::{run, GatewayConfig};
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

#[derive(Debug, Parser)]
#[command(name = "macbotd", version, about = "Mac Bot host daemon")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long)]
    mock: bool,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long, env = "MACBOT_HOME")]
    home: Option<PathBuf>,
    #[arg(long, env = "MACBOT_CONFIG")]
    config: Option<PathBuf>,
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
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
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
    let user_home = user_home();
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| user_home.join("mac-bot/config.toml"));
    let file_config = load_or_create_config(&config_path)?;
    let config_path = std::fs::canonicalize(config_path)?;
    let home = args
        .home
        .clone()
        .or_else(|| {
            file_config
                .data_home
                .as_deref()
                .map(|value| resolve_data_home(value, &user_home, &config_path))
        })
        .unwrap_or_else(|| user_home.join("MacBot"));
    if let Some(command) = args.command {
        return run_command(command, home).await;
    }
    let env_port = if args.port.is_none() {
        env_port()?
    } else {
        None
    };
    let explicit_port = args.port.or(env_port).or(file_config.port);
    if explicit_port == Some(7788) {
        mark_explicit_legacy_port(&home)?;
    } else if explicit_port.is_none() && !args.mock {
        migrate_legacy_port(&home)?;
    }
    let saved = std::fs::read_to_string(home.join("data/settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let saved_port = saved
        .as_ref()
        .and_then(|value| value.get("port").and_then(|v| v.as_u64()))
        .and_then(|value| u16::try_from(value).ok())
        .filter(|port| *port != 0);
    let port = resolve_port(args.port, env_port, file_config.port, saved_port, args.mock);
    let saved_host_name = saved
        .as_ref()
        .and_then(|value| value.get("host_name").and_then(|v| v.as_str()));
    let (host_name, host_name_source) = if let Some(host_name) = file_config.host_name.as_deref() {
        (host_name.to_owned(), "config")
    } else if let Some(host_name) = saved_host_name {
        (host_name.to_owned(), "settings")
    } else {
        ("Mac Bot".to_owned(), "default")
    };
    run(GatewayConfig {
        bind_addr: SocketAddr::from(([0, 0, 0, 0], port.port)),
        home,
        password: args.password,
        mock: args.mock,
        host_name,
        host_name_source: host_name_source.to_string(),
        port_source: port.source.to_string(),
        config_path,
    })
    .await
}

#[derive(Debug, Default)]
struct FileConfig {
    port: Option<u16>,
    data_home: Option<String>,
    host_name: Option<String>,
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_or_create_config(
    path: &Path,
) -> Result<FileConfig, Box<dyn std::error::Error + Send + Sync>> {
    let parent = config_parent(path);
    std::fs::create_dir_all(parent)?;
    let _creation_lock = lock_config_creation(parent)?;
    if !path.exists() {
        let template = concat!(
            "# Mac Bot Server configuration. Restart the server after editing.\n",
            "# Uncomment the values you want to override.\n",
            "# port = 28789\n",
            "# data_home = \"~/MacBot\"\n",
            "# host_name = \"Mac Bot\"\n",
        );
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        temporary.write_all(template.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let text = std::fs::read_to_string(path)?;
    let table: toml::Value = toml::from_str(&text)?;
    let table = table.as_table().ok_or_else(|| {
        format!(
            "configuration root must be a TOML table: {}",
            path.display()
        )
    })?;
    let port = match table.get("port") {
        None => None,
        Some(value) => {
            let port = value
                .as_integer()
                .and_then(|value| u16::try_from(value).ok())
                .filter(|value| *value != 0)
                .ok_or_else(|| {
                    format!(
                        "port must be an integer between 1 and 65535 in {}",
                        path.display()
                    )
                })?;
            Some(port)
        }
    };
    let data_home = table
        .get("data_home")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("data_home must be a string in {}", path.display()))
        })
        .transpose()?;
    let host_name = table
        .get("host_name")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("host_name must be a string in {}", path.display()))
        })
        .transpose()?;
    Ok(FileConfig {
        port,
        data_home,
        host_name,
    })
}

fn resolve_data_home(value: &str, user_home: &Path, config_path: &Path) -> PathBuf {
    if value == "~" {
        return user_home.to_path_buf();
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return user_home.join(relative);
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        config_parent(config_path).join(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortResolution {
    port: u16,
    source: &'static str,
}

fn resolve_port(
    cli: Option<u16>,
    env: Option<u16>,
    config: Option<u16>,
    saved: Option<u16>,
    mock: bool,
) -> PortResolution {
    if let Some(port) = cli {
        return PortResolution {
            port,
            source: "command_line",
        };
    }
    if let Some(port) = env {
        return PortResolution {
            port,
            source: "environment",
        };
    }
    if let Some(port) = config {
        return PortResolution {
            port,
            source: "config",
        };
    }
    if let Some(port) = saved {
        return PortResolution {
            port,
            source: "settings",
        };
    }
    PortResolution {
        port: if mock { 7789 } else { 28789 },
        source: "default",
    }
}

fn env_port() -> Result<Option<u16>, std::io::Error> {
    let Ok(value) = std::env::var("MACBOT_PORT") else {
        return Ok(None);
    };
    let port = value.parse::<u16>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MACBOT_PORT must be an integer between 1 and 65535",
        )
    })?;
    if port == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MACBOT_PORT must be an integer between 1 and 65535",
        ));
    }
    Ok(Some(port))
}

fn config_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn lock_config_creation(parent: &Path) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let lock_path = parent.join(".macbot-config.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(lock_path)?;
    loop {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(not(unix))]
fn lock_config_creation(_parent: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

fn migrate_legacy_port(home: &Path) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let path = home.join("data/settings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(false);
    };
    let Some(settings) = value.as_object_mut() else {
        return Ok(false);
    };
    let already_migrated = settings
        .get("server_migrations")
        .and_then(|migrations| migrations.get("legacy_port_7788_to_28789"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if already_migrated || settings.get("port").and_then(|port| port.as_u64()) != Some(7788) {
        return Ok(false);
    }
    settings.insert("port".into(), json!(28789));
    mark_legacy_port_migration_complete(settings);
    write_json_atomic(&path, &value)?;
    Ok(true)
}

fn mark_explicit_legacy_port(home: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = home.join("data/settings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(());
    };
    let Some(settings) = value.as_object_mut() else {
        return Ok(());
    };
    if settings.get("port").and_then(|port| port.as_u64()) != Some(7788) {
        return Ok(());
    }
    let already_migrated = settings
        .get("server_migrations")
        .and_then(|migrations| migrations.get("legacy_port_7788_to_28789"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !already_migrated {
        mark_legacy_port_migration_complete(settings);
        write_json_atomic(&path, &value)?;
    }
    Ok(())
}

fn mark_legacy_port_migration_complete(settings: &mut serde_json::Map<String, serde_json::Value>) {
    let mut migrations = settings
        .get("server_migrations")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    migrations.insert("legacy_port_7788_to_28789".into(), json!(true));
    settings.insert(
        "server_migrations".into(),
        serde_json::Value::Object(migrations),
    );
}

fn write_json_atomic(
    path: &Path,
    value: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parent = config_parent(path);
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_priority_and_legacy_default_migration_are_stable() {
        assert_eq!(
            resolve_port(Some(7788), Some(3001), Some(3002), Some(3003), false),
            PortResolution {
                port: 7788,
                source: "command_line"
            }
        );
        assert_eq!(
            resolve_port(None, Some(7788), Some(3002), Some(3003), false),
            PortResolution {
                port: 7788,
                source: "environment"
            }
        );
        assert_eq!(
            resolve_port(None, None, Some(7788), Some(3003), false),
            PortResolution {
                port: 7788,
                source: "config"
            }
        );
        assert_eq!(
            resolve_port(None, None, None, Some(3002), false),
            PortResolution {
                port: 3002,
                source: "settings"
            }
        );
        assert_eq!(resolve_port(None, None, None, None, false).port, 28789);
        assert_eq!(resolve_port(None, None, None, None, true).port, 7789);
    }

    #[test]
    fn old_default_port_migrates_once_and_later_explicit_7788_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("data/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(&settings_path, r#"{"port":7788,"host_name":"Legacy"}"#).unwrap();

        assert!(migrate_legacy_port(dir.path()).unwrap());
        let migrated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(migrated["port"], 28789);
        assert_eq!(
            migrated["server_migrations"]["legacy_port_7788_to_28789"],
            true
        );
        assert!(!migrate_legacy_port(dir.path()).unwrap());

        let mut explicit = migrated;
        explicit["port"] = json!(7788);
        write_json_atomic(&settings_path, &explicit).unwrap();
        assert!(!migrate_legacy_port(dir.path()).unwrap());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(saved["port"], 7788);
        assert_eq!(resolve_port(None, None, None, Some(7788), false).port, 7788);
    }

    #[test]
    fn explicit_7788_choice_marks_legacy_migration_without_changing_the_port() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("data/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(&settings_path, r#"{"port":7788,"host_name":"Legacy"}"#).unwrap();

        mark_explicit_legacy_port(dir.path()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(saved["port"], 7788);
        assert_eq!(
            saved["server_migrations"]["legacy_port_7788_to_28789"],
            true
        );
        assert!(!migrate_legacy_port(dir.path()).unwrap());
    }

    #[test]
    fn config_template_is_created_and_parses_as_an_empty_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mac-bot/config.toml");
        let config = load_or_create_config(&path).unwrap();
        assert_eq!(config.port, None);
        assert!(config.data_home.is_none());
        assert_eq!(config.host_name, None);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("port = 28789"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn config_overrides_parse_port_home_and_host_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 28789\ndata_home = \"../MacBotData\"\nhost_name = \"工作站\"\n",
        )
        .unwrap();
        let config = load_or_create_config(&path).unwrap();
        assert_eq!(config.port, Some(28789));
        assert_eq!(config.data_home.as_deref(), Some("../MacBotData"));
        assert_eq!(config.host_name.as_deref(), Some("工作站"));
        assert_eq!(
            resolve_data_home("../MacBotData", dir.path(), &path),
            dir.path().join("../MacBotData")
        );
    }

    #[test]
    fn existing_config_permissions_are_corrected_and_bare_filename_has_a_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "port = 28789\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        load_or_create_config(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(config_parent(Path::new("config.toml")), Path::new("."));
    }

    #[test]
    fn concurrent_config_initialization_creates_one_complete_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mac-bot/config.toml");
        let workers = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || load_or_create_config(&path).unwrap())
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert!(worker.join().unwrap().port.is_none());
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("# Mac Bot Server configuration."));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
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
