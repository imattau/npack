use anyhow::{Context, Result, bail};
use bitcoin_hashes::sha256::Hash as Sha256Hash;
use clap::{CommandFactory, Parser, Subcommand};
use clap_mangen::Man;
use goblin::Object;
use nostr_blossom::prelude::BlossomClient;
use nostr_sdk::prelude::*;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env::consts::{ARCH, OS},
    fs,
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const RELEASE_KIND: u16 = 9900;
const REVOCATION_KIND: u16 = 9901;
const PROTOCOL_VERSION: &str = "1";
const NETWORK_TIMEOUT_SECS: u64 = 15;

type ServiceBackup = (PathBuf, Option<(Vec<u8>, fs::Permissions)>);

#[derive(Parser)]
#[command(name = "npack", version, about = "A Nostr-native package manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Default, Deserialize)]
struct Config {
    #[serde(default)]
    network: NetworkConfig,
    #[serde(default)]
    storage: StorageConfig,
    #[serde(default)]
    trust: TrustConfig,
    #[serde(default)]
    install: InstallConfig,
    #[serde(default)]
    identity: IdentityConfig,
}

#[derive(Debug, Default, Deserialize)]
struct NetworkConfig {
    #[serde(default)]
    relays: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StorageConfig {
    #[serde(default)]
    blossom: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TrustConfig {
    #[serde(default)]
    publishers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InstallConfig {
    #[serde(default)]
    user: bool,
}

#[derive(Debug, Default, Deserialize)]
struct IdentityConfig {
    #[serde(default)]
    pubkey: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Lockfile {
    version: u32,
    packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LockedPackage {
    publisher: String,
    name: String,
    version: String,
    sha256: String,
    dependencies: Vec<Dependency>,
    #[serde(default)]
    conflicts: Vec<Dependency>,
    #[serde(default)]
    runtime_requires: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    Hash {
        artifact: PathBuf,
    },
    Verify {
        target: PathBuf,
    },
    Install {
        target: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "system",
            help = "Install into the current user's ~/.local prefix"
        )]
        user: bool,
        #[arg(
            long,
            conflicts_with = "user",
            help = "Install into the system prefix (the default)"
        )]
        system: bool,
        #[arg(long = "allow-capability")]
        allowed_capabilities: Vec<String>,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(long = "server")]
        servers: Vec<String>,
        #[arg(long = "trusted-publisher")]
        trusted_publishers: Vec<String>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 relay list should be used")]
        pubkey: Option<String>,
        #[arg(long, help = "Write the resolved dependency graph to a lockfile")]
        lockfile: Option<PathBuf>,
        #[arg(long, requires = "lockfile")]
        locked: bool,
        #[arg(long, requires = "locked")]
        offline: bool,
    },
    List {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "system",
            help = "List packages installed in the user's prefix"
        )]
        user: bool,
        #[arg(
            long,
            conflicts_with = "user",
            help = "List packages installed in the system prefix"
        )]
        system: bool,
    },
    VerifyInstalled {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "system",
            help = "Verify packages installed in the user's prefix"
        )]
        user: bool,
        #[arg(
            long,
            conflicts_with = "user",
            help = "Verify packages installed in the system prefix"
        )]
        system: bool,
    },
    ReleaseEvent {
        manifest: PathBuf,
        #[arg(
            long,
            help = "32-byte hex-encoded Nostr secret key; defaults to the registered key"
        )]
        secret_key: Option<String>,
        #[arg(long, default_value_t = 0)]
        created_at: u64,
    },
    VerifyEvent {
        event: PathBuf,
        manifest: PathBuf,
    },
    RevokeEvent {
        event: PathBuf,
        #[arg(
            long,
            help = "32-byte hex-encoded Nostr secret key; defaults to the registered key"
        )]
        secret_key: Option<String>,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value_t = 0)]
        created_at: u64,
    },
    Search {
        query: String,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(long = "trusted-publisher")]
        trusted_publishers: Vec<String>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 relay list should be used")]
        pubkey: Option<String>,
        #[arg(long, help = "Ignore cached results and query relays now")]
        refresh: bool,
        #[arg(long, help = "Do not read or write the local search cache")]
        no_cache: bool,
    },
    Fetch {
        sha256: String,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        output: PathBuf,
    },
    Publish {
        manifest: PathBuf,
        #[arg(
            long,
            help = "32-byte hex-encoded Nostr secret key; defaults to the registered key"
        )]
        secret_key: Option<String>,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(long = "server")]
        servers: Vec<String>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 write relays should be used")]
        pubkey: Option<String>,
    },
    /// Publish a signed Nostr text note
    Announce {
        content: Option<String>,
        #[arg(
            long,
            help = "Nostr secret key in nsec or 32-byte hexadecimal form; defaults to the registered key"
        )]
        secret_key: Option<String>,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(
            long,
            help = "Attach a signed kind:9900 package release event JSON file"
        )]
        release_event: Option<PathBuf>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 write relays should be used")]
        pubkey: Option<String>,
    },
    /// Generate a new publisher key and register it in the OS credential store
    GenerateKey {
        #[arg(
            long,
            help = "Print the generated nsec; keep it private and back it up securely"
        )]
        show_secret: bool,
    },
    Register {
        #[arg(
            help = "Nostr secret key in nsec or 32-byte hexadecimal form",
            conflicts_with = "stdin"
        )]
        secret_key: Option<String>,
        #[arg(long, help = "Read the secret key from standard input")]
        stdin: bool,
    },
    Init {
        directory: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        #[arg(long, help = "Publisher npub or hexadecimal public key")]
        publisher: String,
        #[arg(long, help = "Target operating system; defaults to the host OS")]
        os: Option<String>,
        #[arg(long, help = "Target architecture; defaults to the host architecture")]
        arch: Option<String>,
    },
    /// Render the complete command reference as a man page
    Man,
    #[command(alias = "update")]
    InstallRef {
        package: Option<String>,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(long = "server")]
        servers: Vec<String>,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "system",
            help = "Install into the current user's ~/.local prefix"
        )]
        user: bool,
        #[arg(
            long,
            conflicts_with = "user",
            help = "Install into the system prefix (the default)"
        )]
        system: bool,
        #[arg(long = "trusted-publisher")]
        trusted_publishers: Vec<String>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 relay list should be used")]
        pubkey: Option<String>,
        #[arg(long, help = "Write the resolved dependency graph to a lockfile")]
        lockfile: Option<PathBuf>,
        #[arg(long, requires = "lockfile")]
        locked: bool,
        #[arg(long, requires = "locked")]
        offline: bool,
        #[arg(long = "allow-capability")]
        allowed_capabilities: Vec<String>,
    },
    Pack {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Remove {
        package: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with = "system",
            help = "Remove a package from the user's prefix"
        )]
        user: bool,
        #[arg(
            long,
            conflicts_with = "user",
            help = "Remove a package from the system prefix"
        )]
        system: bool,
    },
    Inspect {
        artifact: PathBuf,
    },
    Manifest {
        artifact: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    publisher: String,
    name: String,
    version: String,
    artifact: PathBuf,
    sha256: String,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    conflicts: Vec<Dependency>,
    #[serde(default)]
    artifact_event: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default = "default_os")]
    os: String,
    #[serde(default = "default_arch")]
    arch: String,
    #[serde(default = "default_format")]
    format: String,
    #[serde(default)]
    runtime_requires: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    post_install: Vec<PostInstallAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PostInstallAction {
    action: String,
    path: PathBuf,
}

fn default_os() -> String {
    "any".into()
}
fn default_arch() -> String {
    "any".into()
}
fn default_format() -> String {
    "npk".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Dependency {
    #[serde(default)]
    publisher: Option<String>,
    name: String,
    requirement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct InstalledPackage {
    publisher: String,
    name: String,
    version: String,
    sha256: String,
    artifact: PathBuf,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    conflicts: Vec<Dependency>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    runtime_requires: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = load_config()?;
    match Cli::parse().command {
        Command::Hash { artifact } => println!("{}", hash_file(&artifact)?),
        Command::Verify { target } => {
            let package = if target.extension().and_then(|ext| ext.to_str()) == Some("npk") {
                load_embedded_manifest(&target)?
            } else {
                load_manifest(&target)?
            };
            verify_manifest(&package, &target)?;
            println!(
                "verified {}/{} {}",
                package.publisher, package.name, package.version
            );
        }
        Command::Install {
            target,
            store,
            user,
            system,
            allowed_capabilities,
            relays,
            trusted_publishers,
            pubkey,
            lockfile,
            locked,
            offline,
            servers,
        } => {
            let path = Path::new(&target);
            if path.exists() {
                let package = if path.extension().and_then(|ext| ext.to_str()) == Some("npk") {
                    load_embedded_manifest(path)?
                } else {
                    load_manifest(path)?
                };
                verify_manifest(&package, path)?;
                let installed = install_with_capabilities(
                    &package,
                    path,
                    store.as_deref(),
                    user || (!system && config.install.user),
                    &allowed_capabilities,
                )?;
                println!(
                    "installed {}/{} {}",
                    display_publisher(&installed.publisher),
                    installed.name,
                    installed.version
                );
            } else {
                install_remote_command(
                    &target,
                    None,
                    relays,
                    servers,
                    store,
                    user,
                    system,
                    trusted_publishers,
                    pubkey,
                    lockfile,
                    locked,
                    offline,
                    allowed_capabilities,
                    &config,
                )
                .await?;
            }
        }
        Command::List {
            store,
            user,
            system,
        } => {
            let user = user || (!system && config.install.user);
            for package in installed_packages(Some(&install_paths(store.as_deref(), user).0))? {
                println!(
                    "{}/{} {}",
                    display_publisher(&package.publisher),
                    package.name,
                    package.version
                );
            }
        }
        Command::VerifyInstalled {
            store,
            user,
            system,
        } => {
            verify_installed(store.as_deref(), user || (!system && config.install.user))?;
        }
        Command::ReleaseEvent {
            manifest,
            secret_key,
            created_at,
        } => {
            let secret_key = resolve_secret_key(secret_key.as_deref())?;
            let package = load_manifest(&manifest)?;
            verify_manifest(&package, &manifest)?;
            let timestamp = if created_at == 0 {
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
            } else {
                created_at
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&sign_release_event(
                    &package,
                    &secret_key,
                    timestamp
                )?)?
            );
        }
        Command::VerifyEvent { event, manifest } => {
            let package = load_manifest(&manifest)?;
            let event_json =
                fs::read(&event).with_context(|| format!("reading event {}", event.display()))?;
            let nostr_event: Event =
                serde_json::from_slice(&event_json).context("parsing Nostr event JSON")?;
            verify_release_event(&nostr_event, &package)?;
            println!(
                "verified release event {} for {}/{} {}",
                nostr_event.id,
                display_publisher(&package.publisher),
                package.name,
                package.version
            );
        }
        Command::RevokeEvent {
            event,
            secret_key,
            reason,
            created_at,
        } => {
            let secret_key = resolve_secret_key(secret_key.as_deref())?;
            let event_json = fs::read(&event)?;
            let release: Event = serde_json::from_slice(&event_json)?;
            let timestamp = if created_at == 0 {
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
            } else {
                created_at
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&sign_revocation_event(
                    &release,
                    &secret_key,
                    &reason,
                    timestamp,
                )?)?
            );
        }
        Command::Search {
            query,
            relays,
            trusted_publishers,
            pubkey,
            refresh,
            no_cache,
        } => {
            let relays = configured_relays(relays, &config)?;
            let trusted_publishers = configured_publishers(trusted_publishers, &config)?;
            let pubkey = pubkey.or_else(|| config.identity.pubkey.clone());
            search_releases(
                &query,
                &relays,
                &trusted_publishers,
                pubkey.as_deref(),
                refresh,
                no_cache,
            )
            .await?
        }
        Command::Fetch {
            sha256,
            server,
            output,
        } => {
            let servers = server
                .into_iter()
                .chain(config.storage.blossom.iter().cloned())
                .collect::<Vec<_>>();
            fetch_blob(&sha256, &servers, &output).await?
        }
        Command::Publish {
            manifest,
            secret_key,
            relays,
            servers,
            pubkey,
        } => {
            let secret_key = resolve_secret_key(secret_key.as_deref())?;
            let relays = configured_relays(relays, &config)?;
            let servers = configured_servers(servers, &config)?;
            publish_release(
                &manifest,
                &secret_key,
                &relays,
                &servers,
                pubkey.or_else(|| config.identity.pubkey.clone()).as_deref(),
            )
            .await?
        }
        Command::Announce {
            content,
            secret_key,
            relays,
            release_event,
            pubkey,
        } => {
            let secret_key = resolve_secret_key(secret_key.as_deref())?;
            let relays = configured_relays(relays, &config)?;
            publish_announcement(
                content.as_deref(),
                release_event.as_deref(),
                &secret_key,
                &relays,
                pubkey.or_else(|| config.identity.pubkey.clone()).as_deref(),
            )
            .await?
        }
        Command::GenerateKey { show_secret } => generate_and_register_key(show_secret)?,
        Command::Register { secret_key, stdin } => {
            let secret_key = if stdin {
                let mut secret_key = String::new();
                io::stdin().read_to_string(&mut secret_key)?;
                secret_key.trim().to_owned()
            } else {
                match secret_key {
                    Some(secret_key) => secret_key,
                    None => rpassword::prompt_password("Nostr secret key: ")?,
                }
            };
            register_secret_key(&secret_key)?;
        }
        Command::Init {
            directory,
            name,
            version,
            publisher,
            os,
            arch,
        } => init_package(
            &directory,
            &name,
            &version,
            &publisher,
            os.as_deref().unwrap_or(OS),
            arch.as_deref().unwrap_or(ARCH),
        )?,
        Command::Man => {
            let command = Cli::command();
            Man::new(command).render(&mut io::stdout())?;
        }
        Command::InstallRef {
            package,
            relays,
            servers,
            store,
            user,
            system,
            trusted_publishers,
            pubkey,
            lockfile,
            locked,
            offline,
            allowed_capabilities,
        } => {
            if let Some(package) = package {
                install_remote_command(
                    &package,
                    None,
                    relays,
                    servers,
                    store,
                    user,
                    system,
                    trusted_publishers,
                    pubkey,
                    lockfile,
                    locked,
                    offline,
                    allowed_capabilities,
                    &config,
                )
                .await?
            } else {
                update_all_command(
                    relays,
                    servers,
                    store,
                    user,
                    system,
                    trusted_publishers,
                    pubkey,
                    allowed_capabilities,
                    &config,
                )
                .await?
            }
        }
        Command::Pack { source, output } => pack_npk(&source, &output)?,
        Command::Remove {
            package,
            store,
            user,
            system,
        } => remove_package_at(
            &package,
            store.as_deref(),
            user || (!system && config.install.user),
        )?,
        Command::Inspect { artifact } => inspect_artifact(&artifact)?,
        Command::Manifest { artifact, output } => write_manifest(&artifact, &output)?,
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn install_remote_command(
    package: &str,
    requirement: Option<String>,
    relays: Vec<String>,
    servers: Vec<String>,
    store: Option<PathBuf>,
    user: bool,
    system: bool,
    trusted_publishers: Vec<String>,
    pubkey: Option<String>,
    lockfile: Option<PathBuf>,
    locked: bool,
    offline: bool,
    allowed_capabilities: Vec<String>,
    config: &Config,
) -> Result<()> {
    let relays = if offline {
        Vec::new()
    } else {
        configured_relays(relays, config)?
    };
    let trusted_publishers = configured_publishers(trusted_publishers, config)?;
    let servers = configured_servers(servers, config)?;
    let pubkey = pubkey.or_else(|| config.identity.pubkey.clone());
    let locked_packages = if locked || offline {
        Some(load_lockfile(
            lockfile
                .as_deref()
                .context("--locked/--offline requires --lockfile")?,
        )?)
    } else {
        None
    };
    let (publisher, name) = package
        .split_once('/')
        .map_or((None, package), |(publisher, name)| {
            (Some(normalize_publisher_reference(publisher)), name)
        });
    install_ref(
        name,
        publisher,
        InstallRefOptions {
            relays: &relays,
            store: store.as_deref(),
            user: user || (!system && config.install.user),
            trusted_publishers: &trusted_publishers,
            user_pubkey: pubkey.as_deref(),
            lockfile: lockfile.as_deref(),
            locked_packages: locked_packages.as_ref(),
            blossom_servers: &servers,
            allowed_capabilities: &allowed_capabilities,
            offline,
        },
        requirement,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn update_all_command(
    relays: Vec<String>,
    servers: Vec<String>,
    store: Option<PathBuf>,
    user: bool,
    system: bool,
    trusted_publishers: Vec<String>,
    pubkey: Option<String>,
    allowed_capabilities: Vec<String>,
    config: &Config,
) -> Result<()> {
    let use_user = user || (!system && config.install.user);
    let root = install_paths(store.as_deref(), use_user).0;
    let mut installed = installed_packages(Some(&root))?;
    installed.sort_by(|left, right| {
        installed_package_reference(left).cmp(&installed_package_reference(right))
    });
    if installed.is_empty() {
        println!("No installed packages to update.");
        return Ok(());
    }

    let mut updated = 0;
    for package in installed {
        let reference = installed_package_reference(&package);
        println!("Checking {reference} {}...", package.version);
        let result = install_remote_command(
            &reference,
            Some(format!(">{}", package.version)),
            relays.clone(),
            servers.clone(),
            store.clone(),
            user,
            system,
            trusted_publishers.clone(),
            pubkey.clone(),
            None,
            false,
            false,
            allowed_capabilities.clone(),
            config,
        )
        .await;
        match result {
            Ok(()) => updated += 1,
            Err(error)
                if error.to_string()
                    == format!("no verified release found for {}", package.name) =>
            {
                println!("  up to date");
            }
            Err(error) => return Err(error),
        }
    }
    println!("Updated {updated} package(s).");
    Ok(())
}

async fn connect_with_timeout(client: &Client) -> Result<()> {
    tokio::time::timeout(
        std::time::Duration::from_secs(NETWORK_TIMEOUT_SECS),
        client.connect(),
    )
    .await
    .context("timed out connecting to configured Nostr relays")?;
    Ok(())
}

const KEYRING_SERVICE: &str = "npack";
const KEYRING_ACCOUNT: &str = "publisher";

fn publisher_keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .context("opening the npack OS credential-store entry")
}

fn register_secret_key(secret_key: &str) -> Result<()> {
    let keys = Keys::parse(secret_key).context("secret key must be hex or nsec")?;
    publisher_keyring_entry()?
        .set_password(secret_key)
        .context("storing the publisher key in the OS credential store")?;
    println!(
        "registered publisher {}",
        display_publisher(&keys.public_key().to_hex())
    );
    Ok(())
}

fn generate_and_register_key(show_secret: bool) -> Result<()> {
    let keys = Keys::generate();
    let secret_key = keys.secret_key().to_bech32()?;
    register_secret_key(&secret_key)?;
    if show_secret {
        println!("nsec: {secret_key}");
    } else {
        eprintln!("secret key stored in the OS credential store; use --show-secret to display it");
    }
    Ok(())
}

fn resolve_secret_key(cli_secret_key: Option<&str>) -> Result<String> {
    if let Some(secret_key) = cli_secret_key {
        return Ok(secret_key.to_owned());
    }
    publisher_keyring_entry()?
        .get_password()
        .context("no --secret-key supplied and no registered publisher key was found")
}

fn init_package(
    directory: &Path,
    name: &str,
    version: &str,
    publisher: &str,
    os: &str,
    arch: &str,
) -> Result<()> {
    validate_package_name(name, "package name")?;
    Version::parse(version).context("package version must be valid SemVer")?;
    if publisher.is_empty()
        || publisher.contains('/')
        || publisher.contains('\\')
        || publisher == "."
        || publisher == ".."
    {
        bail!("publisher must be a single safe path component");
    }
    if os.is_empty() || os.chars().any(char::is_control) {
        bail!("target OS must not be empty or contain control characters");
    }
    if arch.is_empty() || arch.chars().any(char::is_control) {
        bail!("target architecture must not be empty or contain control characters");
    }
    let metadata_dir = directory.join(".npack");
    let manifest_path = metadata_dir.join("manifest.json");
    if manifest_path.exists() {
        bail!(
            "package manifest already exists: {}",
            manifest_path.display()
        );
    }
    fs::create_dir_all(&metadata_dir)?;
    let manifest = Manifest {
        publisher: publisher.to_owned(),
        name: name.to_owned(),
        version: version.to_owned(),
        artifact: format!("{name}-{version}-{os}-{arch}.npk").into(),
        sha256: String::new(),
        dependencies: vec![],
        conflicts: vec![],
        artifact_event: None,
        repo: None,
        commit: None,
        os: os.into(),
        arch: arch.into(),
        format: "npk".into(),
        runtime_requires: vec![],
        provides: vec![],
        post_install: vec![],
    };
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    println!("created package scaffold at {}", directory.display());
    println!(
        "add payload files, then run: npack pack {} --output {}",
        directory.display(),
        manifest.artifact.display()
    );
    Ok(())
}

fn inspect_artifact(path: &Path) -> Result<()> {
    if path.extension().and_then(|ext| ext.to_str()) == Some("npk") {
        let manifest = load_embedded_manifest(path)?;
        println!("format: npk");
        println!("publisher: {}", display_publisher(&manifest.publisher));
        println!("name: {}", manifest.name);
        println!("version: {}", manifest.version);
        println!("os: {}", manifest.os);
        println!("arch: {}", manifest.arch);
        println!("sha256: {}", manifest.sha256);
        if manifest.dependencies.is_empty() {
            println!("dependencies: none");
        } else {
            for dependency in &manifest.dependencies {
                let publisher = dependency
                    .publisher
                    .as_deref()
                    .map(display_publisher)
                    .map(|publisher| format!("{publisher}/"))
                    .unwrap_or_default();
                println!(
                    "dependency: {}{} {}",
                    publisher, dependency.name, dependency.requirement
                );
            }
        }
        return Ok(());
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    match Object::parse(&bytes).context("parsing executable")? {
        Object::Elf(elf) => {
            println!("format: elf");
            if let Some(interpreter) = elf.interpreter {
                println!("interpreter: {interpreter}");
            }
            if elf.libraries.is_empty() {
                println!("needed: none");
            } else {
                for library in elf.libraries {
                    println!("needed: {library}");
                }
            }
        }
        Object::PE(_) => println!("format: pe"),
        Object::Mach(_) => println!("format: mach-o"),
        Object::Archive(_) => println!("format: archive"),
        Object::Unknown(magic) => println!("format: unknown ({magic:?})"),
        _ => println!("format: unknown"),
    }
    Ok(())
}

fn write_manifest(artifact: &Path, output: &Path) -> Result<()> {
    if artifact.extension().and_then(|ext| ext.to_str()) != Some("npk") {
        bail!("manifest generation requires an .npk artifact");
    }
    let manifest = load_embedded_manifest(artifact)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("writing manifest {}", output.display()))?;
    println!("wrote manifest {}", output.display());
    Ok(())
}

async fn fetch_blob(sha256: &str, servers: &[String], output: &Path) -> Result<()> {
    let expected: Sha256Hash = sha256
        .parse()
        .context("sha256 must be 64 hexadecimal characters")?;
    if servers.is_empty() {
        bail!("no Blossom servers configured; pass --server or configure [storage].blossom");
    }
    let mut bytes = None;
    for server in servers {
        let candidate = match BlossomClient::new(Url::parse(server)?)
            .get_blob::<Keys>(expected, None, None, None)
            .await
        {
            Ok(candidate) => candidate,
            Err(_) => continue,
        };
        if Sha256Hash::hash(&candidate) == expected {
            bytes = Some(candidate);
            break;
        }
    }
    let bytes = bytes.context("no Blossom server returned the expected SHA-256")?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    println!("fetched {} bytes to {}", bytes.len(), output.display());
    Ok(())
}

fn pack_npk(source: &Path, output: &Path) -> Result<()> {
    if !source.is_dir() {
        bail!("package source must be a directory");
    }
    let embedded_manifest = source.join(".npack/manifest.json");
    if embedded_manifest.exists() {
        let bytes = fs::read(&embedded_manifest)
            .with_context(|| format!("reading {}", embedded_manifest.display()))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("parsing embedded .npack/manifest.json")?;
        validate_manifest_metadata(&manifest, false)?;
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(output)?;
    let encoder = zstd::Encoder::new(file, 3)?;
    let mut archive = tar::Builder::new(encoder);
    let mut entries = Vec::new();
    collect_package_entries(source, &mut entries)?;
    entries.sort();
    let entry_count = entries.len();
    for path in entries {
        let relative = path.strip_prefix(source)?;
        let metadata = fs::symlink_metadata(&path)?;
        let mut header = tar::Header::new_gnu();
        header.set_path(relative)?;
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mode(if metadata.file_type().is_dir() {
            0o755
        } else {
            metadata.permissions().mode() & 0o7777
        });
        if metadata.file_type().is_symlink() {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name(fs::read_link(&path)?)?;
            header.set_size(0);
            header.set_cksum();
            archive.append_data(&mut header, relative, io::empty())?;
        } else if metadata.file_type().is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            archive.append_data(&mut header, relative, io::empty())?;
        } else {
            header.set_size(metadata.len());
            header.set_cksum();
            archive.append_data(&mut header, relative, fs::File::open(&path)?)?;
        }
    }
    let encoder = archive.into_inner()?;
    encoder.finish()?;
    println!("packed {} entries into {}", entry_count, output.display());
    Ok(())
}

fn collect_package_entries(current: &Path, entries: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let path = entry?.path();
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_dir() {
            entries.push(path.clone());
            collect_package_entries(&path, entries)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            entries.push(path);
        } else {
            bail!("unsupported package entry: {}", path.display());
        }
    }
    Ok(())
}

struct InstallRefOptions<'a> {
    relays: &'a [String],
    store: Option<&'a Path>,
    user: bool,
    trusted_publishers: &'a [String],
    user_pubkey: Option<&'a str>,
    lockfile: Option<&'a Path>,
    locked_packages: Option<&'a Lockfile>,
    blossom_servers: &'a [String],
    allowed_capabilities: &'a [String],
    offline: bool,
}

async fn install_ref(
    name: &str,
    publisher: Option<String>,
    options: InstallRefOptions<'_>,
    requirement: Option<String>,
) -> Result<()> {
    let InstallRefOptions {
        relays,
        store,
        user,
        trusted_publishers,
        user_pubkey,
        lockfile,
        locked_packages,
        blossom_servers,
        allowed_capabilities,
        offline,
    } = options;
    let client = Client::default();
    if !offline {
        for relay in relays {
            client.add_relay(relay).await?;
        }
        connect_with_timeout(&client).await?;
        add_user_relays(&client, user_pubkey).await?;
    }
    let (root, prefix) = install_paths(store, user);
    let mut state = ResolverState {
        client: &client,
        allowed_capabilities,
        user,
        trusted_publishers,
        blossom_servers,
        root: &root,
        prefix: &prefix,
        locked_packages,
        visiting: Vec::new(),
        installed: Vec::new(),
        selected: HashMap::new(),
        offline,
    };
    install_remote_package(&mut state, name.to_owned(), publisher, requirement).await?;
    if !offline {
        client.disconnect().await;
    }
    if let Some(lockfile) = locked_packages {
        verify_locked_order(lockfile, &state.installed)?;
        verify_locked_install(lockfile, &root)?;
    }
    if let Some(lockfile) = lockfile {
        write_lockfile(lockfile, &root, &state.installed)?;
    }
    let display_order = state
        .installed
        .iter()
        .map(|package| display_package_reference(package))
        .collect::<Vec<_>>();
    println!("install order: {}", display_order.join(" -> "));
    Ok(())
}

fn write_lockfile(path: &Path, root: &Path, install_order: &[String]) -> Result<()> {
    let installed = installed_packages(Some(root))?;
    let mut packages = Vec::new();
    for key in install_order {
        let package = installed
            .iter()
            .find(|package| {
                let package_key = format!("{}/{}", package.publisher, package.name);
                package_key == *key || package.name == *key
            })
            .with_context(|| format!("installed package missing from lockfile: {key}"))?;
        packages.push(LockedPackage {
            publisher: package.publisher.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            sha256: package.sha256.clone(),
            dependencies: package.dependencies.clone(),
            conflicts: package.conflicts.clone(),
            runtime_requires: package.runtime_requires.clone(),
            provides: package.provides.clone(),
        });
    }
    let lockfile = Lockfile {
        version: 1,
        packages,
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(&lockfile)?)?;
    Ok(())
}

fn load_lockfile(path: &Path) -> Result<Lockfile> {
    let bytes = fs::read(path).with_context(|| format!("reading lockfile {}", path.display()))?;
    let mut lockfile: Lockfile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing lockfile {}", path.display()))?;
    if lockfile.version != 1 {
        bail!("unsupported lockfile version: {}", lockfile.version);
    }
    let mut identities = HashSet::new();
    for package in &mut lockfile.packages {
        package.publisher = normalize_publisher_reference(&package.publisher);
        PublicKey::parse(&package.publisher)
            .with_context(|| format!("invalid lockfile publisher: {}", package.publisher))?;
        Version::parse(&package.version)
            .with_context(|| format!("invalid lockfile version: {}", package.version))?;
        validate_package_name(&package.name, "lockfile package name")?;
        if package.sha256.len() != 64 || !package.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!(
                "invalid lockfile SHA-256 for {}/{}",
                package.publisher,
                package.name
            );
        }
        if !identities.insert(format!("{}/{}", package.publisher, package.name)) {
            bail!(
                "duplicate package in lockfile: {}/{}",
                package.publisher,
                package.name
            );
        }
        for dependency in package
            .dependencies
            .iter_mut()
            .chain(package.conflicts.iter_mut())
        {
            if let Some(publisher) = &mut dependency.publisher {
                *publisher = normalize_publisher_reference(publisher);
                PublicKey::parse(publisher).with_context(|| {
                    format!("invalid lockfile dependency publisher: {publisher}")
                })?;
            }
            VersionReq::parse(&dependency.requirement)
                .with_context(|| format!("invalid lockfile requirement for {}", dependency.name))?;
        }
        validate_dependency_declarations(&package.dependencies)?;
        validate_dependency_declarations(&package.conflicts)?;
        validate_capability_declarations(&package.runtime_requires, "runtime requirement")?;
        validate_capability_declarations(&package.provides, "provided capability")?;
    }
    validate_lockfile_graph(&lockfile)?;
    Ok(lockfile)
}

fn validate_lockfile_graph(lockfile: &Lockfile) -> Result<()> {
    for package in &lockfile.packages {
        for dependency in &package.dependencies {
            let matches = lockfile.packages.iter().filter(|candidate| {
                candidate.name == dependency.name
                    && dependency
                        .publisher
                        .as_deref()
                        .is_none_or(|publisher| candidate.publisher == publisher)
            });
            let candidates = matches.collect::<Vec<_>>();
            let candidate = match candidates.as_slice() {
                [candidate] => candidate,
                [] => bail!(
                    "lockfile dependency is missing: {}/{} depends on {}",
                    package.publisher,
                    package.name,
                    dependency.name
                ),
                _ => bail!(
                    "lockfile dependency is ambiguous: {}/{} depends on {}",
                    package.publisher,
                    package.name,
                    dependency.name
                ),
            };
            let requirement = VersionReq::parse(&dependency.requirement)?;
            let version = Version::parse(&candidate.version)?;
            if !requirement.matches(&version) {
                bail!(
                    "lockfile dependency does not satisfy requirement: {}/{} requires {} {}, locked {}",
                    package.publisher,
                    package.name,
                    dependency.name,
                    dependency.requirement,
                    candidate.version
                );
            }
        }
        for conflict in &package.conflicts {
            let matches = lockfile.packages.iter().any(|candidate| {
                candidate.name == conflict.name
                    && conflict
                        .publisher
                        .as_deref()
                        .is_none_or(|publisher| candidate.publisher == publisher)
                    && VersionReq::parse(&conflict.requirement)
                        .ok()
                        .and_then(|requirement| {
                            Version::parse(&candidate.version)
                                .ok()
                                .map(|version| requirement.matches(&version))
                        })
                        .unwrap_or(false)
            });
            if matches {
                bail!(
                    "lockfile conflict is present: {}/{} conflicts with {}",
                    package.publisher,
                    package.name,
                    conflict.name
                );
            }
        }
    }
    Ok(())
}

fn verify_locked_order(lockfile: &Lockfile, installed: &[String]) -> Result<()> {
    let expected = lockfile
        .packages
        .iter()
        .map(|package| format!("{}/{}", package.publisher, package.name))
        .collect::<Vec<_>>();
    if expected != installed {
        bail!(
            "locked install order differs: expected {}, got {}",
            expected.join(" -> "),
            installed.join(" -> ")
        );
    }
    Ok(())
}

fn verify_locked_install(lockfile: &Lockfile, root: &Path) -> Result<()> {
    let installed = installed_packages(Some(root))?;
    if installed.len() != lockfile.packages.len() {
        bail!(
            "locked install contains {} packages, expected {}",
            installed.len(),
            lockfile.packages.len()
        );
    }
    for locked in &lockfile.packages {
        let found = installed.iter().any(|package| {
            package.publisher == locked.publisher
                && package.name == locked.name
                && package.version == locked.version
                && package.sha256 == locked.sha256
                && package.dependencies == locked.dependencies
                && package.conflicts == locked.conflicts
                && package.runtime_requires == locked.runtime_requires
                && package.provides == locked.provides
        });
        if !found {
            bail!(
                "locked package was not installed: {}/{} {}",
                locked.publisher,
                locked.name,
                locked.version
            );
        }
    }
    Ok(())
}

use std::future::Future;
use std::pin::Pin;

struct ResolverState<'a> {
    client: &'a Client,
    allowed_capabilities: &'a [String],
    user: bool,
    trusted_publishers: &'a [String],
    blossom_servers: &'a [String],
    root: &'a Path,
    prefix: &'a Path,
    locked_packages: Option<&'a Lockfile>,
    visiting: Vec<String>,
    installed: Vec<String>,
    selected: HashMap<String, Manifest>,
    offline: bool,
}

fn cached_release_path(root: &Path, package: &LockedPackage) -> PathBuf {
    root.join("metadata")
        .join("releases")
        .join(&package.publisher)
        .join(&package.name)
        .join(format!("{}-{}.json", package.version, package.sha256))
}

fn cache_release(root: &Path, event: &Event) -> Result<()> {
    let name = tag_value(event, "name").context("release has no name")?;
    validate_package_name(name, "release package name")?;
    let version = tag_value(event, "version").context("release has no version")?;
    let sha256 = tag_value(event, "x").context("release has no artifact hash")?;
    let directory = root
        .join("metadata")
        .join("releases")
        .join(event.pubkey.to_hex())
        .join(name);
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join(format!("{version}-{sha256}.json")),
        serde_json::to_vec_pretty(event)?,
    )?;
    Ok(())
}

fn load_cached_release(root: &Path, package: &LockedPackage) -> Result<Event> {
    let path = cached_release_path(root, package);
    let event: Event = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("reading cached release {}", path.display()))?,
    )?;
    event
        .verify()
        .context("cached release has invalid signature")?;
    if !release_event_is_v1(&event)
        || event.pubkey.to_hex() != package.publisher
        || tag_value(&event, "name") != Some(package.name.as_str())
        || tag_value(&event, "version") != Some(package.version.as_str())
        || tag_value(&event, "x") != Some(package.sha256.as_str())
    {
        bail!("cached release does not match the lockfile");
    }
    Ok(event)
}

fn cached_artifact_path(root: &Path, event_id: &EventId) -> PathBuf {
    root.join("metadata")
        .join("artifacts")
        .join(format!("{}.json", event_id.to_hex()))
}

fn cache_artifact(root: &Path, event: &Event) -> Result<()> {
    let path = cached_artifact_path(root, &event.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(event)?)?;
    Ok(())
}

fn load_cached_artifact(root: &Path, event_id: EventId) -> Result<Event> {
    let path = cached_artifact_path(root, &event_id);
    let event: Event = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("reading cached artifact {}", path.display()))?,
    )?;
    event
        .verify()
        .context("cached artifact has invalid signature")?;
    if event.kind.as_u16() != 1063 {
        bail!("cached artifact is not a NIP-94 event");
    }
    Ok(event)
}

fn install_remote_package<'a, 'b>(
    state: &'a mut ResolverState<'b>,
    name: String,
    publisher: Option<String>,
    requirement: Option<String>,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let install_key = publisher.as_deref().map_or_else(
            || name.to_owned(),
            |publisher| format!("{publisher}/{name}"),
        );
        if state
            .visiting
            .iter()
            .any(|visiting_name| visiting_name == &install_key)
        {
            bail!(
                "dependency cycle detected: {} -> {}",
                state.visiting.join(" -> "),
                name
            );
        }
        if state
            .installed
            .iter()
            .any(|installed_name| installed_name == &install_key)
        {
            return Ok(());
        }
        let selected = state.selected.iter().find(|(key, manifest)| {
            key.as_str() == install_key || (publisher.is_none() && manifest.name == name)
        });
        if let Some((key, manifest)) = selected {
            if let Some(requirement) = &requirement {
                let requirement = VersionReq::parse(requirement)?;
                let version = Version::parse(&manifest.version)?;
                if !requirement.matches(&version) {
                    bail!("selected package {key} does not satisfy requirement {requirement}");
                }
            }
            return Ok(());
        }
        if !state.trusted_publishers.is_empty()
            && publisher.as_deref().is_some_and(|publisher| {
                !state
                    .trusted_publishers
                    .iter()
                    .any(|trusted| trusted == publisher)
            })
        {
            bail!("publisher is not in the trusted publisher list");
        }
        let locked_package = if let Some(lockfile) = state.locked_packages {
            let matches = lockfile
                .packages
                .iter()
                .filter(|package| {
                    package.name == name
                        && publisher
                            .as_deref()
                            .is_none_or(|publisher| package.publisher == publisher)
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [package] => Some(*package),
                [] => None,
                _ => bail!("package {install_key} is ambiguous in the lockfile"),
            }
        } else {
            None
        };
        if state.locked_packages.is_some() && locked_package.is_none() {
            bail!("package {install_key} is not present in the lockfile");
        }
        state.visiting.push(install_key.clone());
        let release = if state.offline {
            let package = locked_package.context("offline installs require a lockfile")?;
            let release = load_cached_release(state.root, package)?;
            if let Some(requirement) = &requirement {
                let requirement = VersionReq::parse(requirement)?;
                let version = Version::parse(
                    tag_value(&release, "version").context("cached release has no version")?,
                )?;
                if !requirement.matches(&version) {
                    bail!("cached release does not satisfy requirement {requirement}");
                }
            }
            release
        } else {
            let releases = state
                .client
                .fetch_events(Filter::new().kind(Kind::Custom(RELEASE_KIND)).limit(500))
                .timeout(std::time::Duration::from_secs(10))
                .await?;
            let revoked = state
                .client
                .fetch_events(Filter::new().kind(Kind::Custom(REVOCATION_KIND)).limit(500))
                .timeout(std::time::Duration::from_secs(10))
                .await?
                .into_iter()
                .filter(|event| event.verify().is_ok())
                .filter(revocation_event_is_v1)
                .filter_map(|event| {
                    tag_value(&event, "e")
                        .map(|release| (event.pubkey.to_hex(), release.to_owned()))
                })
                .collect::<Vec<_>>();
            releases
                .into_iter()
                .filter(|event| event.verify().is_ok())
                .filter(release_event_is_v1)
                .filter(|event| {
                    state.trusted_publishers.is_empty()
                        || state
                            .trusted_publishers
                            .iter()
                            .any(|trusted| trusted == &event.pubkey.to_hex())
                })
                .filter(|event| {
                    !revoked.iter().any(|(publisher, release_id)| {
                        publisher == &event.pubkey.to_hex() && release_id == &event.id.to_hex()
                    })
                })
                .filter(|event| tag_value(event, "name") == Some(name.as_str()))
                .filter(release_matches_host)
                .filter(|event| {
                    publisher
                        .as_deref()
                        .is_none_or(|publisher| event.pubkey.to_hex() == publisher)
                })
                .filter(|event| {
                    locked_package.is_none_or(|package| {
                        event.pubkey.to_hex() == package.publisher
                            && tag_value(event, "version") == Some(package.version.as_str())
                            && tag_value(event, "x") == Some(package.sha256.as_str())
                    })
                })
                .filter(|event| {
                    requirement.as_deref().is_none_or(|req| {
                        VersionReq::parse(req)
                            .ok()
                            .zip(tag_value(event, "version").and_then(|v| Version::parse(v).ok()))
                            .map(|(req, version)| req.matches(&version))
                            .unwrap_or(false)
                    })
                })
                .max_by_key(|event| {
                    (
                        tag_value(event, "version")
                            .and_then(|v| Version::parse(v).ok())
                            .unwrap_or_else(|| Version::new(0, 0, 0)),
                        event.created_at.as_secs(),
                        event.id.to_hex(),
                    )
                })
                .with_context(|| format!("no verified release found for {name}"))?
        };
        if !state.offline {
            cache_release(state.root, &release)?;
        }
        let artifact_event_id = tag_value(&release, "artifact")
            .context("release has no artifact event")?
            .parse::<EventId>()?;
        let artifact_event = if state.offline {
            load_cached_artifact(state.root, artifact_event_id)?
        } else {
            let artifact_events = state
                .client
                .fetch_events(Filter::new().kind(Kind::Custom(1063)).id(artifact_event_id))
                .timeout(std::time::Duration::from_secs(10))
                .await?;
            let artifact_event = artifact_events
                .into_iter()
                .find(|event| event.verify().is_ok())
                .context("no verified NIP-94 artifact event found")?;
            cache_artifact(state.root, &artifact_event)?;
            artifact_event
        };
        if artifact_event.pubkey != release.pubkey {
            bail!("release and artifact event publishers do not match");
        }
        let sha256 = tag_value(&release, "x").context("release has no artifact hash")?;
        validate_artifact_event(&artifact_event, &release.pubkey, sha256)?;
        let expected: Sha256Hash = sha256.parse()?;
        let staging = state.root.join("downloads").join(sha256);
        fs::create_dir_all(&staging)?;
        let artifact_path = staging.join("artifact");
        if !artifact_path.exists() {
            if state.offline {
                bail!("offline artifact cache is missing for SHA-256 {sha256}");
            }
            let publisher_blossom_servers = discover_blossom_servers(state.client, &release.pubkey)
                .await
                .unwrap_or_default();
            let mut urls = artifact_event
                .tags
                .iter()
                .filter(|tag| tag.kind() == "url")
                .filter_map(Tag::content)
                .map(str::to_owned)
                .collect::<Vec<String>>();
            urls.extend(publisher_blossom_servers);
            urls.extend(state.blossom_servers.iter().cloned());
            urls.sort();
            urls.dedup();
            if urls.is_empty() {
                bail!("artifact event has no URL");
            }
            let mut bytes = None;
            for url in urls {
                let mut server_url = match Url::parse(&url) {
                    Ok(url) => url,
                    Err(_) => continue,
                };
                server_url.set_path("/");
                server_url.set_query(None);
                server_url.set_fragment(None);
                let candidate = match BlossomClient::new(server_url)
                    .get_blob::<Keys>(expected, None, None, None)
                    .await
                {
                    Ok(candidate) => candidate,
                    Err(_) => continue,
                };
                if Sha256Hash::hash(&candidate) == expected {
                    bytes = Some(candidate);
                    break;
                }
            }
            fs::write(
                &artifact_path,
                bytes.context("no artifact mirror returned the expected SHA-256")?,
            )?;
        }
        if Sha256Hash::hash(&fs::read(&artifact_path)?) != expected {
            bail!("cached artifact hash does not match release");
        }
        let manifest = manifest_from_release(&release, &artifact_path, sha256)?;
        let canonical_key = format!("{}/{}", manifest.publisher, manifest.name);
        for (selected_key, selected_manifest) in &state.selected {
            if manifest
                .conflicts
                .iter()
                .any(|conflict| manifest_satisfies_dependency(selected_manifest, conflict))
                || selected_manifest
                    .conflicts
                    .iter()
                    .any(|conflict| manifest_satisfies_dependency(&manifest, conflict))
            {
                bail!("resolved package conflict between {canonical_key} and {selected_key}");
            }
        }
        state.selected.insert(canonical_key, manifest.clone());
        let dependencies = manifest.dependencies.clone();
        for dependency in dependencies {
            install_remote_package(
                state,
                dependency.name.clone(),
                dependency.publisher,
                Some(dependency.requirement),
            )
            .await?;
        }
        install_with_capabilities_at(
            &manifest,
            &staging.join("manifest.json"),
            Some(state.root),
            Some(state.prefix),
            state.user,
            state.allowed_capabilities,
        )?;
        state.visiting.pop();
        state.installed.push(install_key);
        Ok(())
    })
}

fn tag_value<'a>(event: &'a Event, kind: &str) -> Option<&'a str> {
    event
        .tags
        .iter()
        .find(|tag| tag.kind() == kind)
        .and_then(Tag::content)
}

fn release_matches_host(event: &Event) -> bool {
    let os = tag_value(event, "os").unwrap_or("any");
    let arch = tag_value(event, "arch").unwrap_or("any");
    (os == "any" || os == OS) && (arch == "any" || arch == ARCH)
}

async fn add_user_relays(client: &Client, user_pubkey: Option<&str>) -> Result<()> {
    let Some(user_pubkey) = user_pubkey else {
        return Ok(());
    };
    let pubkey = PublicKey::parse(user_pubkey)
        .with_context(|| "NIP-65 pubkey must be an npub or 64-character hexadecimal key")?;
    let events = client
        .fetch_events(
            Filter::new()
                .kind(Kind::Custom(10002))
                .author(pubkey)
                .limit(1),
        )
        .timeout(std::time::Duration::from_secs(10))
        .await?;
    let Some(event) = events
        .into_iter()
        .filter(|event| event.verify().is_ok())
        .max_by_key(|event| event.created_at.as_secs())
    else {
        return Ok(());
    };
    for tag in event.tags.iter().filter(|tag| tag.kind() == "r") {
        let values = tag.clone().to_vec();
        let is_write_only = values.get(2).is_some_and(|marker| marker == "write");
        if !is_write_only && let Some(relay) = values.get(1) {
            client.add_relay(relay).await?;
        }
    }
    connect_with_timeout(client).await?;
    Ok(())
}

async fn add_user_write_relays(client: &Client, user_pubkey: Option<&str>) -> Result<()> {
    let Some(user_pubkey) = user_pubkey else {
        return Ok(());
    };
    let pubkey = PublicKey::parse(user_pubkey)
        .with_context(|| "NIP-65 pubkey must be an npub or 64-character hexadecimal key")?;
    let events = client
        .fetch_events(
            Filter::new()
                .kind(Kind::Custom(10002))
                .author(pubkey)
                .limit(1),
        )
        .timeout(std::time::Duration::from_secs(10))
        .await?;
    let Some(event) = events
        .into_iter()
        .filter(|event| event.verify().is_ok())
        .max_by_key(|event| event.created_at.as_secs())
    else {
        return Ok(());
    };
    for tag in event.tags.iter().filter(|tag| tag.kind() == "r") {
        let values = tag.clone().to_vec();
        let is_read_only = values.get(2).is_some_and(|marker| marker == "read");
        if !is_read_only && let Some(relay) = values.get(1) {
            client.add_relay(relay).await?;
        }
    }
    connect_with_timeout(client).await?;
    Ok(())
}

async fn discover_blossom_servers(client: &Client, publisher: &PublicKey) -> Result<Vec<String>> {
    let events = client
        .fetch_events(
            Filter::new()
                .kind(Kind::Custom(10063))
                .author(*publisher)
                .limit(10),
        )
        .timeout(std::time::Duration::from_secs(10))
        .await?;
    let Some(event) = events
        .into_iter()
        .filter(|event| event.verify().is_ok())
        .max_by_key(|event| (event.created_at.as_secs(), event.id.to_hex()))
    else {
        return Ok(Vec::new());
    };
    let mut servers = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "server")
        .filter_map(Tag::content)
        .filter_map(|server| {
            let server = server.trim();
            (!server.is_empty()).then(|| server.trim_end_matches('/').to_owned())
        })
        .collect::<Vec<_>>();
    servers.sort();
    servers.dedup();
    Ok(servers)
}

fn manifest_from_release(event: &Event, artifact: &Path, sha256: &str) -> Result<Manifest> {
    for kind in [
        "d", "name", "version", "os", "arch", "format", "x", "artifact", "repo", "commit",
    ] {
        let count = event.tags.iter().filter(|tag| tag.kind() == kind).count();
        if count > 1 {
            bail!("release event contains duplicate {kind} tags");
        }
    }
    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        match tag.kind() {
            "depends" | "conflicts" if values.len() != 3 && values.len() != 4 => {
                bail!("invalid {} tag in release event", tag.kind());
            }
            "post-install" if values.len() != 3 => {
                bail!("invalid post-install tag in release event");
            }
            _ => {}
        }
    }
    let dependency_tags = event.tags.iter().filter(|tag| tag.kind() == "depends");
    let dependencies = dependency_tags
        .filter_map(|tag| {
            let values = tag.clone().to_vec();
            (values.len() >= 3).then(|| Dependency {
                publisher: (values.len() >= 4).then(|| values[1].clone()),
                name: if values.len() >= 4 {
                    values[2].clone()
                } else {
                    values[1].clone()
                },
                requirement: if values.len() >= 4 {
                    values[3].clone()
                } else {
                    values[2].clone()
                },
            })
        })
        .collect::<Vec<_>>();
    let conflicts = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "conflicts")
        .filter_map(|tag| {
            let values = tag.clone().to_vec();
            (values.len() >= 3).then(|| Dependency {
                publisher: (values.len() >= 4).then(|| values[1].clone()),
                name: if values.len() >= 4 {
                    values[2].clone()
                } else {
                    values[1].clone()
                },
                requirement: if values.len() >= 4 {
                    values[3].clone()
                } else {
                    values[2].clone()
                },
            })
        })
        .collect::<Vec<_>>();
    let runtime_requires = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "requires")
        .filter_map(Tag::content)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let provides = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "provides")
        .filter_map(Tag::content)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let post_install = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "post-install")
        .filter_map(|tag| {
            let values = tag.clone().to_vec();
            (values.len() >= 3).then(|| PostInstallAction {
                action: values[1].clone(),
                path: values[2].clone().into(),
            })
        })
        .collect();
    validate_dependency_declarations(&dependencies)?;
    validate_dependency_declarations(&conflicts)?;
    validate_capability_declarations(&runtime_requires, "runtime requirement")?;
    validate_capability_declarations(&provides, "provided capability")?;
    let repo = tag_value(event, "repo").map(str::to_owned);
    if let Some(repo) = &repo {
        validate_repo_reference(repo)?;
    }
    let commit = tag_value(event, "commit").map(str::to_owned);
    if let Some(commit) = &commit
        && (commit.is_empty() || commit.chars().any(char::is_whitespace))
    {
        bail!("release commit must be a non-empty commit identifier");
    }
    Ok(Manifest {
        publisher: event.pubkey.to_hex(),
        name: tag_value(event, "name")
            .context("release has no name")?
            .into(),
        version: tag_value(event, "version")
            .context("release has no version")?
            .into(),
        artifact: artifact
            .file_name()
            .context("artifact has no filename")?
            .into(),
        sha256: sha256.into(),
        dependencies,
        conflicts,
        artifact_event: tag_value(event, "artifact").map(str::to_owned),
        repo,
        commit,
        os: tag_value(event, "os").unwrap_or("any").into(),
        arch: tag_value(event, "arch").unwrap_or("any").into(),
        format: tag_value(event, "format").unwrap_or("opaque").into(),
        runtime_requires,
        provides,
        post_install,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchCache {
    created_at: u64,
    releases: Vec<Event>,
    revocations: Vec<Event>,
}

fn revocation_pairs(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| {
            tag_value(event, "e").map(|release| (event.pubkey.to_hex(), release.to_owned()))
        })
        .collect()
}

fn search_cache_path(
    query: &str,
    relays: &[String],
    trusted_publishers: &[String],
    user_pubkey: Option<&str>,
) -> PathBuf {
    let key =
        serde_json::to_vec(&(query, relays, trusted_publishers, user_pubkey)).unwrap_or_default();
    let digest = Sha256::digest(key);
    default_store()
        .join("search-cache")
        .join(format!("{}.json", hex::encode(digest)))
}

fn load_search_cache(path: &Path, max_age: u64) -> Result<Option<SearchCache>> {
    let cache: SearchCache = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing search cache {}", path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if now.saturating_sub(cache.created_at) > max_age {
        return Ok(None);
    }
    Ok(Some(cache))
}

fn save_search_cache(path: &Path, cache: &SearchCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec(cache)?)?;
    Ok(())
}

async fn search_releases(
    query: &str,
    relays: &[String],
    trusted_publishers: &[String],
    user_pubkey: Option<&str>,
    refresh: bool,
    no_cache: bool,
) -> Result<()> {
    const SEARCH_CACHE_MAX_AGE_SECS: u64 = 300;
    let cache_path = search_cache_path(query, relays, trusted_publishers, user_pubkey);
    let cached = if !refresh && !no_cache {
        load_search_cache(&cache_path, SEARCH_CACHE_MAX_AGE_SECS).unwrap_or(None)
    } else {
        None
    };
    let (events, revoked) = if let Some(cache) = cached {
        let age = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|now| now.as_secs().saturating_sub(cache.created_at))
            .unwrap_or_default();
        eprintln!("Using cached search results ({age}s old)");
        let revoked = revocation_pairs(&cache.revocations);
        (cache.releases, revoked)
    } else {
        eprint!("Searching {} Nostr relay(s)...", relays.len());
        io::stderr().flush()?;
        let started = Instant::now();
        let client = Client::default();
        for relay in relays {
            client
                .add_relay(relay)
                .await
                .with_context(|| format!("adding relay {relay}"))?;
        }
        connect_with_timeout(&client).await?;
        add_user_relays(&client, user_pubkey).await?;
        let filter = Filter::new().kind(Kind::Custom(RELEASE_KIND)).limit(500);
        let events = client
            .fetch_events(filter)
            .timeout(std::time::Duration::from_secs(10))
            .await
            .context("querying Nostr relays")?;
        let revocation_events = client
            .fetch_events(Filter::new().kind(Kind::Custom(REVOCATION_KIND)).limit(500))
            .timeout(std::time::Duration::from_secs(10))
            .await?
            .into_iter()
            .filter(|event| event.verify().is_ok())
            .filter(revocation_event_is_v1)
            .collect::<Vec<_>>();
        let revoked = revocation_pairs(&revocation_events);
        if !no_cache {
            let cache = SearchCache {
                created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                releases: events.iter().cloned().collect(),
                revocations: revocation_events,
            };
            let _ = save_search_cache(&cache_path, &cache);
        }
        eprintln!(" done in {:.1}s", started.elapsed().as_secs_f32());
        client.disconnect().await;
        (events.into_iter().collect(), revoked)
    };
    let query = query.to_ascii_lowercase();
    let mut latest_versions = HashMap::<(String, String), Version>::new();
    let mut matches = Vec::new();
    for event in events {
        if !trusted_publishers.is_empty()
            && !trusted_publishers
                .iter()
                .any(|trusted| trusted == &event.pubkey.to_hex())
        {
            continue;
        }
        if revoked.iter().any(|(publisher, release_id)| {
            publisher == &event.pubkey.to_hex() && release_id == &event.id.to_hex()
        }) {
            continue;
        }
        let name_matches = event.tags.iter().any(|tag| {
            tag.kind() == "name"
                && tag
                    .content()
                    .map(|name| name.to_ascii_lowercase().contains(&query))
                    .unwrap_or(false)
        });
        if !name_matches || event.verify().is_err() || !release_event_is_v1(&event) {
            continue;
        }
        let Some(name) = tag_value(&event, "name") else {
            continue;
        };
        let Some(version_text) = tag_value(&event, "version") else {
            continue;
        };
        let Ok(version) = Version::parse(version_text) else {
            continue;
        };
        let key = (event.pubkey.to_hex(), name.to_owned());
        if latest_versions
            .get(&key)
            .is_none_or(|latest| &version >= latest)
        {
            latest_versions.insert(key, version);
        }
        matches.push(event);
    }
    for event in matches {
        let name = tag_value(&event, "name").unwrap_or_default();
        let version = tag_value(&event, "version").unwrap_or_default();
        let key = (event.pubkey.to_hex(), name.to_owned());
        if latest_versions
            .get(&key)
            .is_some_and(|latest| latest == &Version::parse(version).unwrap())
        {
            println!(
                "{} {} {}",
                event.id,
                display_publisher(&event.pubkey.to_hex()),
                event.content
            );
        }
    }
    Ok(())
}

fn verify_release_event(event: &Event, manifest: &Manifest) -> Result<()> {
    if event.kind.as_u16() != RELEASE_KIND {
        bail!(
            "expected npack release event kind {RELEASE_KIND}, got {}",
            event.kind
        );
    }
    event
        .verify()
        .context("invalid Nostr event ID or signature")?;
    if let Ok(publisher) = PublicKey::parse(&manifest.publisher)
        && event.pubkey != publisher
    {
        bail!("release event signer does not match manifest publisher");
    }
    let expected_artifact = manifest
        .artifact_event
        .as_deref()
        .context("release manifests must include an artifact event")?;
    let expected_d = format!("{}/{}/{}", manifest.name, manifest.version, manifest.arch);
    let actual_d = tag_value(event, "d").context("release event is missing d tag")?;
    if actual_d != expected_d {
        bail!("release event d mismatch: expected {expected_d}, got {actual_d}");
    }
    for (kind, expected) in [
        ("v", PROTOCOL_VERSION),
        ("name", manifest.name.as_str()),
        ("version", manifest.version.as_str()),
        ("os", manifest.os.as_str()),
        ("arch", manifest.arch.as_str()),
        ("format", manifest.format.as_str()),
        ("x", manifest.sha256.to_ascii_lowercase().as_str()),
        ("artifact", expected_artifact),
    ] {
        let actual = event
            .tags
            .iter()
            .find(|tag| tag.kind() == kind)
            .and_then(Tag::content)
            .with_context(|| format!("release event is missing {kind} tag"))?;
        if actual != expected {
            bail!("release event {kind} mismatch: expected {expected}, got {actual}");
        }
    }
    let event_manifest = manifest_from_release(event, &manifest.artifact, &manifest.sha256)?;
    if event_manifest.dependencies != manifest.dependencies
        || event_manifest.conflicts != manifest.conflicts
        || event_manifest.runtime_requires != manifest.runtime_requires
        || event_manifest.provides != manifest.provides
        || event_manifest.post_install != manifest.post_install
        || event_manifest.artifact_event != manifest.artifact_event
        || event_manifest.repo != manifest.repo
        || event_manifest.commit != manifest.commit
    {
        bail!("release event metadata does not match the package manifest");
    }
    Ok(())
}

fn release_event_is_v1(event: &Event) -> bool {
    if event.kind.as_u16() != RELEASE_KIND || tag_value(event, "v") != Some(PROTOCOL_VERSION) {
        return false;
    }
    if [
        "d", "v", "name", "version", "os", "arch", "format", "x", "artifact", "repo", "commit",
    ]
    .iter()
    .any(|kind| event.tags.iter().filter(|tag| tag.kind() == *kind).count() > 1)
    {
        return false;
    }
    let Some(hash) = tag_value(event, "x") else {
        return false;
    };
    hash.len() == 64
        && hash.chars().all(|character| character.is_ascii_hexdigit())
        && ["d", "name", "version", "os", "arch", "format", "artifact"]
            .iter()
            .all(|kind| tag_value(event, kind).is_some())
}

fn revocation_event_is_v1(event: &Event) -> bool {
    if event.kind.as_u16() != REVOCATION_KIND
        || tag_value(event, "v") != Some(PROTOCOL_VERSION)
        || ["v", "e", "name", "version", "x", "reason"]
            .iter()
            .any(|kind| event.tags.iter().filter(|tag| tag.kind() == *kind).count() != 1)
    {
        return false;
    }
    let Some(release_id) = tag_value(event, "e") else {
        return false;
    };
    if release_id.parse::<EventId>().is_err() {
        return false;
    }
    let Some(name) = tag_value(event, "name") else {
        return false;
    };
    if validate_package_name(name, "revocation package name").is_err() {
        return false;
    }
    tag_value(event, "version").is_some_and(|version| Version::parse(version).is_ok())
        && tag_value(event, "x")
            .is_some_and(|hash| hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        && tag_value(event, "reason").is_some_and(|reason| !reason.trim().is_empty())
}

fn validate_artifact_event(event: &Event, publisher: &PublicKey, sha256: &str) -> Result<()> {
    if event.kind.as_u16() != 1063 {
        bail!("expected NIP-94 artifact event kind:1063");
    }
    event
        .verify()
        .context("invalid NIP-94 artifact event signature")?;
    if &event.pubkey != publisher {
        bail!("artifact event publisher does not match release publisher");
    }
    let artifact_hash = tag_value(event, "x").context("artifact event has no SHA-256 tag")?;
    if artifact_hash != sha256 || artifact_hash.len() != 64 || !artifact_hash.is_ascii() {
        bail!("artifact event SHA-256 does not match release");
    }
    if hex::decode(artifact_hash).is_err() {
        bail!("artifact event SHA-256 is not valid hexadecimal");
    }
    let mime = tag_value(event, "m").context("artifact event has no MIME type")?;
    if mime != "application/zstd" {
        bail!("artifact event MIME type must be application/zstd");
    }
    if !event.tags.iter().any(|tag| tag.kind() == "url") {
        bail!("artifact event has no download URL");
    }
    Ok(())
}

fn sign_release_event(manifest: &Manifest, secret_hex: &str, created_at: u64) -> Result<Event> {
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    if let Ok(publisher) = PublicKey::parse(&manifest.publisher)
        && keys.public_key() != publisher
    {
        bail!("release signer does not match manifest publisher");
    }
    let artifact_event = manifest
        .artifact_event
        .as_deref()
        .context("release manifests must include an artifact event")?;
    let mut tags = vec![
        vec![
            "d".into(),
            format!("{}/{}/{}", manifest.name, manifest.version, manifest.arch),
        ],
        vec!["v".into(), PROTOCOL_VERSION.into()],
        vec!["name".into(), manifest.name.clone()],
        vec!["version".into(), manifest.version.clone()],
        vec!["os".into(), manifest.os.clone()],
        vec!["arch".into(), manifest.arch.clone()],
        vec!["format".into(), manifest.format.clone()],
        vec!["x".into(), manifest.sha256.to_ascii_lowercase()],
    ];
    tags.push(vec!["artifact".into(), artifact_event.into()]);
    if let Some(repo) = &manifest.repo {
        tags.push(vec!["repo".into(), repo.clone()]);
    }
    if let Some(commit) = &manifest.commit {
        tags.push(vec!["commit".into(), commit.clone()]);
    }
    for dependency in &manifest.dependencies {
        let mut tag = vec!["depends".into()];
        if let Some(publisher) = &dependency.publisher {
            tag.push(publisher.clone());
        }
        tag.push(dependency.name.clone());
        tag.push(dependency.requirement.clone());
        tags.push(tag);
    }
    for conflict in &manifest.conflicts {
        let mut tag = vec!["conflicts".into()];
        if let Some(publisher) = &conflict.publisher {
            tag.push(publisher.clone());
        }
        tag.push(conflict.name.clone());
        tag.push(conflict.requirement.clone());
        tags.push(tag);
    }
    for requirement in &manifest.runtime_requires {
        tags.push(vec!["requires".into(), requirement.clone()]);
    }
    for provided in &manifest.provides {
        tags.push(vec!["provides".into(), provided.clone()]);
    }
    for action in &manifest.post_install {
        tags.push(vec![
            "post-install".into(),
            action.action.clone(),
            action.path.to_string_lossy().into_owned(),
        ]);
    }
    let content = format!(
        "npack release {}/{} {}",
        manifest.publisher, manifest.name, manifest.version
    );
    let tags = tags
        .into_iter()
        .map(Tag::parse)
        .collect::<Result<Vec<_>, _>>()?;
    EventBuilder::new(Kind::Custom(RELEASE_KIND), content)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys)
        .map_err(Into::into)
}

async fn publish_release(
    manifest_path: &Path,
    secret_hex: &str,
    relays: &[String],
    servers: &[String],
    user_pubkey: Option<&str>,
) -> Result<()> {
    let manifest = load_manifest(manifest_path)?;
    verify_manifest(&manifest, manifest_path)?;
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    let publisher = PublicKey::parse(&manifest.publisher)
        .context("publish manifest publisher must be an npub or hex public key")?;
    if keys.public_key() != publisher {
        bail!("publish signer does not match manifest publisher");
    }
    let bytes = fs::read(artifact_path(&manifest, manifest_path))?;
    let expected = Sha256Hash::hash(&bytes);
    let mut descriptor = None;
    let mut failures = Vec::new();
    for server in servers {
        let upload = tokio::time::timeout(
            std::time::Duration::from_secs(NETWORK_TIMEOUT_SECS),
            BlossomClient::new(Url::parse(server)?).upload_blob(
                bytes.clone(),
                Some("application/zstd".into()),
                None,
                Some(&keys),
            ),
        )
        .await;
        match upload {
            Ok(Ok(candidate)) if candidate.sha256 == expected => {
                descriptor = Some(candidate);
                break;
            }
            Ok(Ok(candidate)) => failures.push(format!(
                "{server}: returned hash {} instead of {}",
                candidate.sha256, expected
            )),
            Ok(Err(error)) => failures.push(format!("{server}: {error}")),
            Err(_) => failures.push(format!("{server}: timed out")),
        }
    }
    let descriptor = descriptor.with_context(|| {
        format!(
            "no Blossom server accepted the artifact upload; attempts: {}",
            failures.join("; ")
        )
    })?;
    let artifact_event =
        sign_artifact_event(&manifest, descriptor.url.as_ref(), bytes.len(), &keys)?;
    let mut release_manifest = manifest.clone();
    release_manifest.artifact_event = Some(artifact_event.id.to_hex());
    let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let release_event = sign_release_event(&release_manifest, secret_hex, created_at)?;

    let client = Client::default();
    for relay in relays {
        client.add_relay(relay).await?;
    }
    connect_with_timeout(&client).await?;
    add_user_write_relays(&client, user_pubkey).await?;
    client
        .send_event(&artifact_event)
        .await
        .context("publishing NIP-94 artifact event")?;
    client
        .send_event(&release_event)
        .await
        .context("publishing package release event")?;
    client.disconnect().await;
    println!("artifact event: {}", artifact_event.id);
    println!("release event: {}", release_event.id);
    Ok(())
}

async fn publish_announcement(
    content: Option<&str>,
    release_event_path: Option<&Path>,
    secret_hex: &str,
    relays: &[String],
    user_pubkey: Option<&str>,
) -> Result<()> {
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    let release = release_event_path
        .map(|path| -> Result<Event> {
            let event: Event = serde_json::from_slice(
                &fs::read(path)
                    .with_context(|| format!("reading release event {}", path.display()))?,
            )?;
            if event.kind.as_u16() != RELEASE_KIND || !release_event_is_v1(&event) {
                bail!("release event is not a valid npack kind:{RELEASE_KIND} event");
            }
            event
                .verify()
                .context("release event has invalid signature")?;
            Ok(event)
        })
        .transpose()?;
    let (content, tags) = if let Some(release) = &release {
        let package = tag_value(release, "name").context("release has no package name")?;
        let version = tag_value(release, "version").context("release has no version")?;
        let event_uri = release.to_nostr_uri()?;
        let generated = format!(
            "npack release {package} {version}\n\nPackage: {package}\nVersion: {version}\nOS: {}\nArchitecture: {}\nSHA-256: {}\n\nRelease event: {event_uri}",
            tag_value(release, "os").unwrap_or("any"),
            tag_value(release, "arch").unwrap_or("any"),
            tag_value(release, "x").unwrap_or("unknown"),
        );
        let content = content.map(str::to_owned).unwrap_or(generated);
        let tags = vec![
            Tag::parse(vec![
                "e".into(),
                release.id.to_hex(),
                "".into(),
                "mention".into(),
            ])?,
            Tag::parse(vec![String::from("t"), String::from("npack")])?,
            Tag::parse(vec![String::from("t"), package.to_owned()])?,
        ];
        (content, tags)
    } else {
        let content = content.context("announcement text or --release-event is required")?;
        (
            content.to_owned(),
            vec![Tag::parse(vec![String::from("t"), String::from("npack")])?],
        )
    };
    let event = EventBuilder::new(Kind::TextNote, content)
        .tags(tags)
        .finalize(&keys)?;
    let client = Client::default();
    for relay in relays {
        client.add_relay(relay).await?;
    }
    connect_with_timeout(&client).await?;
    add_user_write_relays(&client, user_pubkey).await?;
    client
        .send_event(&event)
        .await
        .context("publishing Nostr announcement")?;
    client.disconnect().await;
    println!("announcement event: {}", event.id);
    println!(
        "publisher: {}",
        display_publisher(&keys.public_key().to_hex())
    );
    Ok(())
}

fn sign_artifact_event(manifest: &Manifest, url: &str, size: usize, keys: &Keys) -> Result<Event> {
    let tags = vec![
        Tag::parse(vec!["url".to_owned(), url.to_owned()])?,
        Tag::parse(vec!["m".to_owned(), "application/zstd".to_owned()])?,
        Tag::parse(vec!["x".to_owned(), manifest.sha256.to_ascii_lowercase()])?,
        Tag::parse(vec!["size".to_owned(), size.to_string()])?,
    ];
    EventBuilder::new(Kind::Custom(1063), manifest.name.clone())
        .tags(tags)
        .finalize(keys)
        .map_err(Into::into)
}

fn sign_revocation_event(
    release: &Event,
    secret_hex: &str,
    reason: &str,
    created_at: u64,
) -> Result<Event> {
    if release.kind.as_u16() != RELEASE_KIND {
        bail!("can only revoke a kind:{RELEASE_KIND} release event");
    }
    if !release_event_is_v1(release) {
        bail!("can only revoke a valid npack protocol v{PROTOCOL_VERSION} release event");
    }
    release
        .verify()
        .context("release event has invalid signature")?;
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    if keys.public_key() != release.pubkey {
        bail!("revocation signer must match the release publisher");
    }
    let tags = vec![
        Tag::parse(vec!["v".to_owned(), PROTOCOL_VERSION.to_owned()])?,
        Tag::parse(vec!["e".into(), release.id.to_hex()])?,
        Tag::parse(vec![
            String::from("name"),
            tag_value(release, "name").unwrap_or_default().to_owned(),
        ])?,
        Tag::parse(vec![
            String::from("version"),
            tag_value(release, "version").unwrap_or_default().to_owned(),
        ])?,
        Tag::parse(vec![
            String::from("x"),
            tag_value(release, "x").unwrap_or_default().to_owned(),
        ])?,
        Tag::parse(vec![String::from("reason"), reason.to_owned()])?,
    ];
    EventBuilder::new(Kind::Custom(REVOCATION_KIND), reason)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys)
        .map_err(Into::into)
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let bytes = fs::read(path).with_context(|| format!("reading manifest {}", path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).context("parsing JSON manifest")?;
    validate_manifest_metadata(&manifest, true)?;
    Ok(manifest)
}

fn validate_manifest_metadata(manifest: &Manifest, require_hash: bool) -> Result<()> {
    if manifest.publisher.is_empty() || manifest.name.is_empty() || manifest.version.is_empty() {
        bail!("manifest publisher, name, and version must not be empty");
    }
    for (label, value) in [
        ("publisher", &manifest.publisher),
        ("name", &manifest.name),
        ("version", &manifest.version),
    ] {
        if value.contains('/') || value.contains('\\') || value == "." || value == ".." {
            bail!("manifest {label} must be a single safe path component");
        }
    }
    if manifest.artifact.as_os_str().is_empty()
        || manifest.artifact.is_absolute()
        || manifest.artifact.components().count() != 1
    {
        bail!("manifest artifact must be a single safe filename");
    }
    if require_hash
        && (manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()))
    {
        bail!("manifest sha256 must be 64 hexadecimal characters");
    }
    validate_dependency_declarations(&manifest.dependencies)?;
    validate_dependency_declarations(&manifest.conflicts)?;
    validate_capability_declarations(&manifest.runtime_requires, "runtime requirement")?;
    validate_capability_declarations(&manifest.provides, "provided capability")?;
    if let Some(repo) = &manifest.repo {
        validate_repo_reference(repo)?;
    }
    if let Some(commit) = &manifest.commit
        && (commit.is_empty() || commit.chars().any(char::is_whitespace))
    {
        bail!("manifest commit must be a non-empty commit identifier");
    }
    Ok(())
}

fn load_embedded_manifest(archive_path: &Path) -> Result<Manifest> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("reading package {}", archive_path.display()))?;
    let decoder = zstd::Decoder::new(file).context("opening .npk zstd stream")?;
    let mut archive = tar::Archive::new(decoder);
    let mut manifest = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(".npack/manifest.json") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            manifest = Some(
                serde_json::from_slice::<Manifest>(&bytes)
                    .context("parsing embedded .npack/manifest.json")?,
            );
            break;
        }
    }
    let mut manifest = manifest.context(".npk does not contain .npack/manifest.json")?;
    manifest.artifact = archive_path
        .file_name()
        .context("package path has no filename")?
        .into();
    manifest.sha256 = hash_file(archive_path)?;
    validate_manifest_metadata(&manifest, true)?;
    Ok(manifest)
}

fn validate_repo_reference(repo: &str) -> Result<()> {
    let mut parts = repo.splitn(3, ':');
    let kind = parts.next();
    let publisher = parts.next();
    let identifier = parts.next();
    if kind != Some("30617") {
        bail!("manifest repo must be a NIP-34 kind:30617 address");
    }
    let publisher = publisher.context("manifest repo is missing its publisher")?;
    PublicKey::parse(publisher).context("manifest repo publisher is not a valid Nostr key")?;
    let identifier = identifier.context("manifest repo is missing its identifier")?;
    validate_package_name(identifier, "manifest repo identifier")?;
    Ok(())
}

fn validate_package_name(name: &str, label: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        bail!("{label} must be a single safe path component");
    }
    Ok(())
}

fn validate_dependency_declarations(dependencies: &[Dependency]) -> Result<()> {
    for dependency in dependencies {
        validate_package_name(&dependency.name, "dependency name")?;
        if let Some(publisher) = &dependency.publisher
            && (publisher.is_empty()
                || publisher.contains('/')
                || publisher.contains('\\')
                || publisher == "."
                || publisher == "..")
        {
            bail!("dependency publisher must be a single safe path component");
        }
        VersionReq::parse(&dependency.requirement).with_context(|| {
            format!(
                "invalid dependency version requirement for {}: {}",
                dependency.name, dependency.requirement
            )
        })?;
    }
    Ok(())
}

fn validate_capability_declarations(capabilities: &[String], label: &str) -> Result<()> {
    for capability in capabilities {
        if capability.trim().is_empty() || capability.chars().any(char::is_control) {
            bail!("{label} must not be empty or contain control characters");
        }
    }
    Ok(())
}

fn artifact_path(manifest: &Manifest, manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&manifest.artifact)
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading artifact {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn verify_manifest(manifest: &Manifest, manifest_path: &Path) -> Result<()> {
    let artifact = artifact_path(manifest, manifest_path);
    let actual = hash_file(&artifact)?;
    if actual != manifest.sha256.to_ascii_lowercase() {
        bail!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            artifact.display(),
            manifest.sha256,
            actual
        );
    }
    if let Some(required) = elf_libraries(&artifact)? {
        for library in required {
            if !manifest
                .runtime_requires
                .iter()
                .any(|declared| declared == &library)
            {
                bail!("ELF dependency {library} is not declared in runtime_requires");
            }
        }
    }
    Ok(())
}

fn elf_libraries(path: &Path) -> Result<Option<Vec<String>>> {
    let bytes = fs::read(path)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Ok(None);
    }
    match Object::parse(&bytes).context("parsing executable")? {
        Object::Elf(elf) => Ok(Some(
            elf.libraries
                .iter()
                .map(|library| (*library).into())
                .collect(),
        )),
        _ => Ok(None),
    }
}

fn default_store() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("npack")
}

fn load_config() -> Result<Config> {
    let Some(config_dir) = dirs::config_dir() else {
        return Ok(Config::default());
    };
    let path = config_dir.join("npack/config.toml");
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("reading npack config {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parsing npack config {}", path.display()))
}

fn configured_relays(cli_relays: Vec<String>, config: &Config) -> Result<Vec<String>> {
    let relays = if cli_relays.is_empty() {
        config.network.relays.clone()
    } else {
        cli_relays
    };
    let mut relays = relays
        .into_iter()
        .map(|relay| relay.trim().trim_end_matches('/').to_owned())
        .filter(|relay| !relay.is_empty())
        .collect::<Vec<_>>();
    relays.sort();
    relays.dedup();
    if relays.is_empty() {
        bail!("no relays configured; pass --relay or configure [network].relays");
    }
    Ok(relays)
}

fn configured_servers(cli_servers: Vec<String>, config: &Config) -> Result<Vec<String>> {
    let mut servers = if cli_servers.is_empty() {
        config.storage.blossom.clone()
    } else {
        cli_servers
    }
    .into_iter()
    .map(|server| server.trim().trim_end_matches('/').to_owned())
    .filter(|server| !server.is_empty())
    .collect::<Vec<_>>();
    servers.sort();
    servers.dedup();
    if servers.is_empty() {
        bail!("no Blossom servers configured; pass --server or configure [storage].blossom");
    }
    Ok(servers)
}

fn configured_publishers(cli_publishers: Vec<String>, config: &Config) -> Result<Vec<String>> {
    let publishers = if cli_publishers.is_empty() {
        config.trust.publishers.clone()
    } else {
        cli_publishers
    };
    publishers
        .iter()
        .map(|publisher| {
            PublicKey::parse(publisher)
                .map(|key| key.to_hex())
                .with_context(|| format!("invalid trusted publisher key: {publisher}"))
        })
        .collect()
}

fn normalize_publisher_reference(publisher: &str) -> String {
    PublicKey::parse(publisher)
        .map(|key| key.to_hex())
        .unwrap_or_else(|_| publisher.to_owned())
}

fn display_publisher(publisher: &str) -> String {
    PublicKey::parse(publisher)
        .map(|key| {
            key.to_bech32()
                .expect("public key bech32 encoding is infallible")
        })
        .unwrap_or_else(|_| publisher.to_owned())
}

fn display_package_reference(package: &str) -> String {
    package
        .split_once('/')
        .map(|(publisher, name)| format!("{}/{}", display_publisher(publisher), name))
        .unwrap_or_else(|| package.to_owned())
}

fn installed_package_reference(package: &InstalledPackage) -> String {
    format!("{}/{}", display_publisher(&package.publisher), package.name)
}

fn default_system_store() -> PathBuf {
    PathBuf::from("/var/lib/npack")
}

fn install_paths(store: Option<&Path>, user: bool) -> (PathBuf, PathBuf) {
    if let Some(store) = store {
        return (store.to_path_buf(), store.join("packages"));
    }
    if user {
        let state = default_store();
        let prefix = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local");
        (state, prefix)
    } else {
        (default_system_store(), PathBuf::from("/"))
    }
}

#[cfg(test)]
fn install(
    manifest: &Manifest,
    manifest_path: &Path,
    store: Option<&Path>,
) -> Result<InstalledPackage> {
    install_with_capabilities(manifest, manifest_path, store, false, &[])
}

fn install_with_capabilities(
    manifest: &Manifest,
    manifest_path: &Path,
    store: Option<&Path>,
    user: bool,
    allowed_capabilities: &[String],
) -> Result<InstalledPackage> {
    install_with_capabilities_at(
        manifest,
        manifest_path,
        store,
        None,
        user,
        allowed_capabilities,
    )
}

fn install_with_capabilities_at(
    manifest: &Manifest,
    manifest_path: &Path,
    store: Option<&Path>,
    prefix_override: Option<&Path>,
    user: bool,
    allowed_capabilities: &[String],
) -> Result<InstalledPackage> {
    let (root, default_prefix) = install_paths(store, user);
    let prefix = prefix_override.unwrap_or(&default_prefix);
    ensure_dependencies_available(manifest, &root)?;
    let package_dir = root
        .join("packages")
        .join(&manifest.publisher)
        .join(&manifest.name)
        .join(&manifest.version);
    fs::create_dir_all(&package_dir)
        .with_context(|| format!("creating {}", package_dir.display()))?;
    let destination = package_dir.join(
        manifest
            .artifact
            .file_name()
            .context("artifact must have a filename")?,
    );
    fs::copy(artifact_path(manifest, manifest_path), &destination)
        .context("copying verified artifact")?;
    let existing_packages = installed_packages(Some(&root))?;
    if manifest.format == "npk" {
        let payload_paths = npk_entry_paths(&destination)?
            .into_iter()
            .map(|path| prefix.join(path))
            .collect::<Vec<_>>();
        ensure_install_paths_available(
            &payload_paths,
            &existing_packages,
            &manifest.publisher,
            &manifest.name,
            &manifest.version,
        )?;
    }
    validate_post_install_actions(&manifest.post_install, allowed_capabilities)?;
    let files = if manifest.format == "npk" {
        let staging = package_dir.join("payload");
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        extract_npk(&destination, &staging)?;
        install_staged_npk(&staging, prefix, &npk_entry_paths(&destination)?)?
    } else {
        vec![destination.clone()]
    };
    let mut files = files;
    files.extend(run_post_install(
        &manifest.post_install,
        prefix,
        user,
        allowed_capabilities,
    )?);
    let installed = InstalledPackage {
        publisher: manifest.publisher.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        sha256: manifest.sha256.to_ascii_lowercase(),
        artifact: destination,
        dependencies: manifest.dependencies.clone(),
        conflicts: manifest.conflicts.clone(),
        files,
        runtime_requires: manifest.runtime_requires.clone(),
        provides: manifest.provides.clone(),
    };
    remove_stale_files(&existing_packages, &installed)?;
    let mut packages = existing_packages;
    packages.retain(|p| !(p.publisher == installed.publisher && p.name == installed.name));
    packages.push(installed.clone());
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("installed.json"),
        serde_json::to_vec_pretty(&packages)?,
    )?;
    Ok(installed)
}

fn remove_stale_files(existing: &[InstalledPackage], replacement: &InstalledPackage) -> Result<()> {
    let previous_files = existing
        .iter()
        .filter(|package| {
            package.publisher == replacement.publisher && package.name == replacement.name
        })
        .flat_map(|package| package.files.iter())
        .cloned()
        .collect::<HashSet<_>>();
    if previous_files.is_empty() {
        return Ok(());
    }
    for file in previous_files {
        if replacement
            .files
            .iter()
            .any(|replacement_file| replacement_file == &file)
        {
            continue;
        }
        let owned_by_other_package = existing.iter().any(|package| {
            !(package.publisher == replacement.publisher && package.name == replacement.name)
                && package.files.iter().any(|other_file| other_file == &file)
        });
        if owned_by_other_package {
            continue;
        }
        if file.is_file()
            || fs::symlink_metadata(&file).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::remove_file(&file)?;
        }
    }
    Ok(())
}

fn run_post_install(
    actions: &[PostInstallAction],
    prefix: &Path,
    user: bool,
    allowed_capabilities: &[String],
) -> Result<Vec<PathBuf>> {
    validate_post_install_actions(actions, allowed_capabilities)?;
    let mut created = Vec::new();
    let mut created_directories = Vec::new();
    let mut service_backups: Vec<ServiceBackup> = Vec::new();
    for action in actions {
        let result = (|| -> Result<()> {
            match action.action.as_str() {
                "create-directory" => {
                    let destination = prefix.join(&action.path);
                    if !destination.exists() {
                        fs::create_dir_all(&destination)?;
                        created_directories.push(destination);
                    }
                }
                "register-service" => {
                    let source = prefix.join(&action.path);
                    if source.extension().and_then(|extension| extension.to_str())
                        != Some("service")
                    {
                        bail!("register-service requires a .service file");
                    }
                    let destination = if user {
                        dirs::config_dir()
                            .context("cannot determine user config directory")?
                            .join("systemd/user")
                    } else {
                        PathBuf::from("/etc/systemd/system")
                    }
                    .join(source.file_name().context("service path has no filename")?);
                    fs::create_dir_all(destination.parent().unwrap())?;
                    let backup = match fs::symlink_metadata(&destination) {
                        Ok(metadata) if metadata.file_type().is_file() => {
                            Some((fs::read(&destination)?, metadata.permissions()))
                        }
                        Ok(_) => bail!(
                            "service destination is not a regular file: {}",
                            destination.display()
                        ),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.into()),
                    };
                    atomic_copy_service(&source, &destination)?;
                    service_backups.push((destination.clone(), backup));
                    created.push(destination);
                }
                _ => bail!("unsupported post-install action: {}", action.action),
            }
            Ok(())
        })();
        if let Err(error) = result {
            rollback_post_install(&created, &created_directories, &service_backups)?;
            return Err(error);
        }
    }
    Ok(created)
}

fn atomic_copy_service(source: &Path, destination: &Path) -> Result<()> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary =
        destination.with_extension(format!("service.npack-{}-{}", std::process::id(), nonce));
    let result = (|| -> Result<()> {
        fs::copy(source, &temporary)?;
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn rollback_post_install(
    created: &[PathBuf],
    created_directories: &[PathBuf],
    service_backups: &[ServiceBackup],
) -> Result<()> {
    for path in created.iter().rev() {
        if path.exists() || fs::symlink_metadata(path).is_ok() {
            fs::remove_file(path)?;
        }
    }
    for (path, backup) in service_backups.iter().rev() {
        if let Some((contents, permissions)) = backup {
            fs::write(path, contents)?;
            fs::set_permissions(path, permissions.clone())?;
        }
    }
    for directory in created_directories.iter().rev() {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_post_install_actions(
    actions: &[PostInstallAction],
    allowed_capabilities: &[String],
) -> Result<()> {
    for action in actions {
        let capability = if action.action == "create-directory" {
            "filesystem:install-prefix"
        } else if action.action == "register-service" {
            "service-manager"
        } else {
            action.action.as_str()
        };
        if action.action != "create-directory"
            && !allowed_capabilities
                .iter()
                .any(|allowed| allowed == capability)
        {
            bail!("post-install action requires capability: {capability}");
        }
        if action.path.is_absolute()
            || action
                .path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("unsafe post-install path: {}", action.path.display());
        }
        if action.action == "register-service"
            && action
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("service")
        {
            bail!("register-service requires a .service file");
        }
        if action.action != "create-directory" && action.action != "register-service" {
            bail!("unsupported post-install action: {}", action.action);
        }
    }
    Ok(())
}

fn npk_entry_paths(archive_path: &Path) -> Result<Vec<PathBuf>> {
    let file = fs::File::open(archive_path)?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let relative = entry.path()?.into_owned();
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("unsafe path in .npk archive: {}", relative.display());
        }
        if !seen.insert(relative.clone()) {
            bail!("duplicate path in .npk archive: {}", relative.display());
        }
        if relative == Path::new(".npack") || relative.starts_with(".npack/") {
            continue;
        }
        if entry.header().entry_type().is_symlink() {
            let target = entry.link_name()?.context("symlink entry has no target")?;
            if target.is_absolute()
                || target
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                bail!(
                    "unsafe symlink target in .npk archive: {}",
                    target.display()
                );
            }
        }
        paths.push(relative);
    }
    Ok(paths)
}

fn install_staged_npk(staging: &Path, prefix: &Path, entries: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut installed = Vec::new();
    let mut applied = Vec::new();
    let mut created_directories = Vec::new();
    let backup_root = staging.join(".npack-backups");
    for relative in entries {
        let result = (|| -> Result<()> {
            let source = staging.join(relative);
            let destination = prefix.join(relative);
            let metadata = fs::symlink_metadata(&source)?;
            if metadata.file_type().is_dir() {
                if !destination.exists() {
                    fs::create_dir_all(&destination)?;
                    created_directories.push(destination.clone());
                }
                return Ok(());
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let backup = if destination.exists() || fs::symlink_metadata(&destination).is_ok() {
                let backup = backup_root.join(applied.len().to_string());
                backup_entry(&destination, &backup)?;
                Some(backup)
            } else {
                None
            };
            applied.push((destination.clone(), backup));
            if destination.exists() || fs::symlink_metadata(&destination).is_ok() {
                fs::remove_file(&destination)?;
            }
            if metadata.file_type().is_symlink() {
                symlink(fs::read_link(&source)?, &destination)?;
            } else if metadata.file_type().is_file() {
                fs::copy(&source, &destination)?;
                fs::set_permissions(&destination, metadata.permissions())?;
            } else {
                bail!("unsupported staged package entry: {}", relative.display());
            }
            installed.push(destination);
            Ok(())
        })();
        if let Err(error) = result {
            rollback_entries(&applied)?;
            rollback_directories(&created_directories)?;
            return Err(error);
        }
    }
    Ok(installed)
}

fn rollback_directories(directories: &[PathBuf]) -> Result<()> {
    for directory in directories.iter().rev() {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn backup_entry(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        symlink(fs::read_link(source)?, destination)?;
    } else if metadata.file_type().is_file() {
        fs::copy(source, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
    } else {
        bail!(
            "cannot back up non-file installation path: {}",
            source.display()
        );
    }
    Ok(())
}

fn rollback_entries(entries: &[(PathBuf, Option<PathBuf>)]) -> Result<()> {
    for (destination, backup) in entries.iter().rev() {
        if destination.exists() || fs::symlink_metadata(destination).is_ok() {
            fs::remove_file(destination)?;
        }
        if let Some(backup) = backup
            && (backup.exists() || fs::symlink_metadata(backup).is_ok())
        {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::symlink_metadata(backup)?.file_type().is_symlink() {
                symlink(fs::read_link(backup)?, destination)?;
            } else {
                fs::copy(backup, destination)?;
                fs::set_permissions(destination, fs::metadata(backup)?.permissions())?;
            }
        }
    }
    Ok(())
}

fn ensure_install_paths_available(
    paths: &[PathBuf],
    installed: &[InstalledPackage],
    publisher: &str,
    name: &str,
    _version: &str,
) -> Result<()> {
    for path in paths {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            continue;
        }
        let owned_by_this_package = installed.iter().any(|package| {
            package.publisher == publisher
                && package.name == name
                && package.files.iter().any(|file| file == path)
        });
        if !owned_by_this_package {
            bail!(
                "refusing to overwrite unowned installed file: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn extract_npk(archive_path: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(destination)?;
    let file = fs::File::open(archive_path)?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let relative = entry.path()?.into_owned();
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("unsafe path in .npk archive: {}", relative.display());
        }
        if !seen.insert(relative.clone()) {
            bail!("duplicate path in .npk archive: {}", relative.display());
        }
        if entry.header().entry_type().is_symlink() {
            let target = entry.link_name()?.context("symlink entry has no target")?;
            if target.is_absolute()
                || target
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                bail!(
                    "unsafe symlink target in .npk archive: {}",
                    target.display()
                );
            }
        }
        let unpacked = entry.unpack_in(destination)?;
        if unpacked {
            files.push(destination.join(relative));
        }
    }
    Ok(files)
}

fn manifest_satisfies_dependency(manifest: &Manifest, dependency: &Dependency) -> bool {
    dependency
        .publisher
        .as_deref()
        .is_none_or(|publisher| publisher == manifest.publisher)
        && dependency.name == manifest.name
        && VersionReq::parse(&dependency.requirement)
            .ok()
            .and_then(|requirement| {
                Version::parse(&manifest.version)
                    .ok()
                    .map(|version| requirement.matches(&version))
            })
            .unwrap_or(false)
}

fn ensure_dependencies_available(manifest: &Manifest, store: &Path) -> Result<()> {
    let installed = installed_packages(Some(store))?;
    let system = system_capabilities();
    for conflict in &manifest.conflicts {
        let requirement = VersionReq::parse(&conflict.requirement).with_context(|| {
            format!(
                "invalid conflict version requirement for {}: {}",
                conflict.name, conflict.requirement
            )
        })?;
        if installed.iter().any(|package| {
            package.name == conflict.name
                && conflict
                    .publisher
                    .as_deref()
                    .is_none_or(|publisher| package.publisher == publisher)
                && Version::parse(&package.version)
                    .map(|version| requirement.matches(&version))
                    .unwrap_or(false)
        }) {
            bail!(
                "package {} conflicts with installed {} {}",
                manifest.name,
                conflict.name,
                conflict.requirement
            );
        }
    }
    for dependency in &manifest.dependencies {
        let requirement = VersionReq::parse(&dependency.requirement).with_context(|| {
            format!(
                "invalid version requirement for {}: {}",
                dependency.name, dependency.requirement
            )
        })?;
        let satisfied = installed.iter().any(|package| {
            package.name == dependency.name
                && dependency
                    .publisher
                    .as_deref()
                    .is_none_or(|publisher| package.publisher == publisher)
                && Version::parse(&package.version)
                    .map(|version| requirement.matches(&version))
                    .unwrap_or(false)
        });
        if !satisfied {
            bail!(
                "missing dependency {} {} (install it before {})",
                dependency.name,
                dependency.requirement,
                manifest.name
            );
        }
    }
    for requirement in &manifest.runtime_requires {
        if !installed.iter().any(|package| {
            package
                .provides
                .iter()
                .any(|provided| capability_satisfies(requirement, provided))
        }) && !system
            .iter()
            .any(|provided| capability_satisfies(requirement, provided))
        {
            bail!(
                "missing runtime capability {requirement} (provide it before {})",
                manifest.name
            );
        }
    }
    Ok(())
}

fn capability_satisfies(requirement: &str, provided: &str) -> bool {
    let Some((name, constraint)) = requirement.split_once(' ') else {
        return requirement == provided;
    };
    let Some((provided_name, provided_version)) = provided.rsplit_once('@') else {
        return false;
    };
    if name != provided_name {
        return false;
    }
    let Ok(requirement) = VersionReq::parse(constraint) else {
        return false;
    };
    let Ok(version) = Version::parse(provided_version) else {
        return false;
    };
    requirement.matches(&version)
}

fn system_capabilities() -> HashSet<String> {
    let mut capabilities = HashSet::from([format!("system:{OS}"), format!("system:{ARCH}")]);
    for directory in host_library_directories() {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if (name.starts_with("lib") || name.starts_with("ld-")) && name.contains(".so") {
                    capabilities.insert(name);
                }
            }
        }
    }
    capabilities
}

fn host_library_directories() -> Vec<PathBuf> {
    let mut directories = vec![
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/usr/lib64"),
    ];
    let multiarch = match ARCH {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "x86" => Some("i386-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        _ => None,
    };
    if let Some(multiarch) = multiarch {
        directories.extend([
            PathBuf::from("/lib").join(multiarch),
            PathBuf::from("/usr/lib").join(multiarch),
        ]);
    }
    directories
}

fn installed_packages(store: Option<&Path>) -> Result<Vec<InstalledPackage>> {
    let root = store
        .map(Path::to_path_buf)
        .unwrap_or_else(default_system_store);
    let path = root.join("installed.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn verify_installed(store: Option<&Path>, user: bool) -> Result<()> {
    let root = install_paths(store, user).0;
    let packages = installed_packages(Some(&root))?;
    for package in &packages {
        let actual = hash_file(&package.artifact).with_context(|| {
            format!(
                "reading artifact for {}/{} {}",
                display_publisher(&package.publisher),
                package.name,
                package.version
            )
        })?;
        if actual != package.sha256.to_ascii_lowercase() {
            bail!(
                "installed artifact hash mismatch for {}/{} {}",
                display_publisher(&package.publisher),
                package.name,
                package.version
            );
        }
        for file in &package.files {
            if fs::symlink_metadata(file).is_err() {
                bail!(
                    "installed file is missing for {}/{} {}: {}",
                    display_publisher(&package.publisher),
                    package.name,
                    package.version,
                    file.display()
                );
            }
        }
        println!(
            "verified {}/{} {}",
            display_publisher(&package.publisher),
            package.name,
            package.version
        );
    }
    Ok(())
}

#[cfg(test)]
fn remove_package(package: &str, store: Option<&Path>) -> Result<()> {
    remove_package_at(package, store, false)
}

fn remove_package_at(package: &str, store: Option<&Path>, user: bool) -> Result<()> {
    let (publisher, name) = package
        .split_once('/')
        .context("package reference must be publisher/name")?;
    let publisher = normalize_publisher_reference(publisher);
    if publisher.is_empty() || name.is_empty() || publisher.contains('/') || name.contains('/') {
        bail!("package reference must be publisher/name");
    }
    let root = install_paths(store, user).0;
    let packages = installed_packages(Some(&root))?;
    if packages.iter().any(|installed| {
        installed.dependencies.iter().any(|dependency| {
            dependency.name == name
                && dependency
                    .publisher
                    .as_deref()
                    .is_none_or(|dependency_publisher| dependency_publisher == publisher)
        })
    }) {
        bail!("cannot remove {package}: it is required by an installed package");
    }
    let removed = packages
        .iter()
        .filter(|installed| installed.publisher == publisher && installed.name == name)
        .collect::<Vec<_>>();
    let remaining_candidates = packages
        .iter()
        .filter(|installed| !(installed.publisher == publisher && installed.name == name))
        .collect::<Vec<_>>();
    let system = system_capabilities();
    for dependent in &remaining_candidates {
        for requirement in &dependent.runtime_requires {
            let target_provides = removed.iter().any(|installed| {
                installed
                    .provides
                    .iter()
                    .any(|provided| capability_satisfies(requirement, provided))
            });
            let alternative_exists = remaining_candidates.iter().any(|installed| {
                installed
                    .provides
                    .iter()
                    .any(|provided| capability_satisfies(requirement, provided))
            }) || system
                .iter()
                .any(|provided| capability_satisfies(requirement, provided));
            if target_provides && !alternative_exists {
                bail!(
                    "cannot remove {package}: it provides runtime capability {requirement} required by {}/{}",
                    dependent.publisher,
                    dependent.name
                );
            }
        }
    }
    let mut remaining = Vec::new();
    let mut removed = 0;
    for installed in &packages {
        if installed.publisher == publisher && installed.name == name {
            for file in &installed.files {
                let owned_by_other_package = packages.iter().any(|other| {
                    !(other.publisher == installed.publisher
                        && other.name == installed.name
                        && other.version == installed.version)
                        && other.files.iter().any(|other_file| other_file == file)
                });
                if owned_by_other_package {
                    continue;
                }
                if (file.is_file()
                    || fs::symlink_metadata(file)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink()))
                    && (file.exists() || fs::symlink_metadata(file).is_ok())
                {
                    fs::remove_file(file)?;
                }
            }
            let package_dir = root
                .join("packages")
                .join(&installed.publisher)
                .join(&installed.name)
                .join(&installed.version);
            if package_dir.exists() {
                fs::remove_dir_all(&package_dir)?;
            }
            removed += 1;
        } else {
            remaining.push(installed.clone());
        }
    }
    if removed == 0 {
        bail!("package {package} is not installed");
    }
    fs::write(
        root.join("installed.json"),
        serde_json::to_vec_pretty(&remaining)?,
    )?;
    println!("removed {package} ({removed} version(s))");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hashes_and_installs_verified_artifact() -> Result<()> {
        let dir = tempdir()?;
        let artifact = dir.path().join("hello.tar.gz");
        fs::write(&artifact, b"hello npack")?;
        let manifest_path = dir.path().join("hello.npack.json");
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.tar.gz".into(),
            sha256: hash_file(&artifact)?,
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: Some("artifact-event-id".into()),
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "opaque".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        verify_manifest(&manifest, &manifest_path)?;
        let store = dir.path().join("store");
        let installed = install(&manifest, &manifest_path, Some(&store))?;
        assert_eq!(installed_packages(Some(&store))?.len(), 1);
        assert!(installed.artifact.exists());
        remove_package("npub1test/hello", Some(&store))?;
        assert!(installed_packages(Some(&store))?.is_empty());
        assert!(!installed.artifact.exists());
        Ok(())
    }

    #[test]
    fn verifies_installed_artifacts_and_files() -> Result<()> {
        let dir = tempdir()?;
        let artifact = dir.path().join("hello.bin");
        fs::write(&artifact, b"hello npack")?;
        let manifest_path = dir.path().join("hello.json");
        let manifest = Manifest {
            publisher: "pub".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.bin".into(),
            sha256: hash_file(&artifact)?,
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "opaque".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        let store = dir.path().join("store");
        install(&manifest, &manifest_path, Some(&store))?;
        verify_installed(Some(&store), false)?;
        let lock_path = dir.path().join("npack.lock.json");
        write_lockfile(&lock_path, &store, &["pub/hello".into()])?;
        let lockfile: Lockfile = serde_json::from_slice(&fs::read(lock_path)?)?;
        assert_eq!(lockfile.version, 1);
        assert_eq!(lockfile.packages[0].sha256, manifest.sha256);
        verify_locked_install(&lockfile, &store)?;
        fs::write(
            store.join("packages/pub/hello/1.0.0/hello.bin"),
            b"tampered",
        )?;
        assert!(verify_installed(Some(&store), false).is_err());
        Ok(())
    }

    #[test]
    fn removes_files_created_outside_the_package_directory() -> Result<()> {
        let dir = tempdir()?;
        let store = dir.path().join("store");
        let package_dir = store.join("packages/pub/service/1.0.0");
        fs::create_dir_all(&package_dir)?;
        let service = dir.path().join("hello.service");
        fs::write(&service, b"[Unit]\nDescription=hello\n")?;
        let installed = InstalledPackage {
            publisher: "pub".into(),
            name: "service".into(),
            version: "1.0.0".into(),
            sha256: "00".repeat(32),
            artifact: package_dir.join("service.npk"),
            dependencies: vec![],
            conflicts: vec![],
            files: vec![service.clone()],
            runtime_requires: vec![],
            provides: vec![],
        };
        fs::create_dir_all(&store)?;
        fs::write(
            store.join("installed.json"),
            serde_json::to_vec(&[installed])?,
        )?;

        remove_package("pub/service", Some(&store))?;

        assert!(!service.exists());
        assert!(!package_dir.exists());
        Ok(())
    }

    #[test]
    fn refuses_overwriting_another_packages_file() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("bin/hello");
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, b"owned")?;
        let installed = InstalledPackage {
            publisher: "other".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            sha256: "00".repeat(32),
            artifact: dir.path().join("artifact.npk"),
            dependencies: vec![],
            conflicts: vec![],
            files: vec![path.clone()],
            runtime_requires: vec![],
            provides: vec![],
        };
        let error = ensure_install_paths_available(
            &[path],
            &[installed],
            "new-publisher",
            "hello",
            "2.0.0",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unowned installed file"));
        Ok(())
    }

    #[test]
    fn rolls_back_partial_host_materialization() -> Result<()> {
        let dir = tempdir()?;
        let staging = dir.path().join("staging");
        fs::create_dir_all(staging.join("bin"))?;
        fs::write(staging.join("bin/one"), b"one")?;
        let prefix = dir.path().join("prefix");
        let error = install_staged_npk(
            &staging,
            &prefix,
            &[
                PathBuf::from("bin"),
                PathBuf::from("bin/one"),
                PathBuf::from("bin/missing"),
            ],
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("No such file") || error.to_string().contains("not found")
        );
        assert!(!prefix.join("bin/one").exists());
        assert!(!prefix.join("bin").exists());
        Ok(())
    }

    #[test]
    fn removal_preserves_files_owned_by_another_package() -> Result<()> {
        let dir = tempdir()?;
        let store = dir.path().join("store");
        let shared = dir.path().join("bin/shared");
        fs::create_dir_all(shared.parent().unwrap())?;
        fs::write(&shared, b"shared")?;
        let packages = vec![
            InstalledPackage {
                publisher: "pub".into(),
                name: "first".into(),
                version: "1.0.0".into(),
                sha256: "00".repeat(32),
                artifact: store.join("first"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![shared.clone()],
                runtime_requires: vec![],
                provides: vec![],
            },
            InstalledPackage {
                publisher: "pub".into(),
                name: "second".into(),
                version: "1.0.0".into(),
                sha256: "11".repeat(32),
                artifact: store.join("second"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![shared.clone()],
                runtime_requires: vec![],
                provides: vec![],
            },
        ];
        fs::create_dir_all(&store)?;
        fs::write(store.join("installed.json"), serde_json::to_vec(&packages)?)?;

        remove_package("pub/first", Some(&store))?;

        assert!(shared.exists());
        Ok(())
    }

    #[test]
    fn reinstall_removes_stale_files() -> Result<()> {
        let dir = tempdir()?;
        let stale = dir.path().join("bin/stale");
        let current = dir.path().join("bin/current");
        fs::create_dir_all(stale.parent().unwrap())?;
        fs::write(&stale, b"stale")?;
        fs::write(&current, b"current")?;
        let previous = InstalledPackage {
            publisher: "pub".into(),
            name: "app".into(),
            version: "1.0.0".into(),
            sha256: "00".repeat(32),
            artifact: dir.path().join("artifact"),
            dependencies: vec![],
            conflicts: vec![],
            files: vec![stale.clone(), current.clone()],
            runtime_requires: vec![],
            provides: vec![],
        };
        let replacement = InstalledPackage {
            files: vec![current.clone()],
            ..previous.clone()
        };
        remove_stale_files(&[previous], &replacement)?;
        assert!(!stale.exists());
        assert!(current.exists());
        Ok(())
    }

    #[test]
    fn upgrade_replaces_previous_version_and_files() -> Result<()> {
        let dir = tempdir()?;
        let old_file = dir.path().join("bin/old");
        let shared_file = dir.path().join("bin/app");
        let new_file = dir.path().join("bin/new");
        fs::create_dir_all(old_file.parent().unwrap())?;
        fs::write(&old_file, b"old")?;
        fs::write(&shared_file, b"v1")?;
        fs::write(&new_file, b"new")?;

        let previous = InstalledPackage {
            publisher: "pub".into(),
            name: "app".into(),
            version: "1.0.0".into(),
            sha256: "00".repeat(32),
            artifact: dir.path().join("old.npk"),
            dependencies: vec![],
            conflicts: vec![],
            files: vec![old_file.clone(), shared_file.clone()],
            runtime_requires: vec![],
            provides: vec![],
        };
        let replacement = InstalledPackage {
            publisher: "pub".into(),
            name: "app".into(),
            version: "2.0.0".into(),
            sha256: "11".repeat(32),
            artifact: dir.path().join("new.npk"),
            dependencies: vec![],
            conflicts: vec![],
            files: vec![shared_file.clone(), new_file.clone()],
            runtime_requires: vec![],
            provides: vec![],
        };

        remove_stale_files(&[previous], &replacement)?;
        assert!(!old_file.exists());
        assert!(shared_file.exists());
        assert!(new_file.exists());
        Ok(())
    }

    #[test]
    fn rejects_tampered_artifact() -> Result<()> {
        let dir = tempdir()?;
        let artifact = dir.path().join("app.bin");
        fs::write(&artifact, b"original")?;
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "app".into(),
            version: "1.0.0".into(),
            artifact: "app.bin".into(),
            sha256: hash_file(&artifact)?,
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "opaque".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(&artifact, b"tampered")?;
        assert!(verify_manifest(&manifest, &dir.path().join("manifest.json")).is_err());
        Ok(())
    }

    #[test]
    fn creates_signed_release_event() -> Result<()> {
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.tar.gz".into(),
            sha256: "00".repeat(32),
            dependencies: vec![Dependency {
                publisher: None,
                name: "libfoo".into(),
                requirement: ">=2".into(),
            }],
            conflicts: vec![],
            artifact_event: Some("artifact-event-id".into()),
            repo: Some(format!("30617:{}:hello", "11".repeat(32))),
            commit: Some("commit-sha".into()),
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "tar.zst".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        let event = sign_release_event(&manifest, &"11".repeat(32), 1_700_000_000)?;
        assert_eq!(event.kind.as_u16(), 9900);
        assert_eq!(event.created_at.as_secs(), 1_700_000_000);
        assert_eq!(event.pubkey.to_hex().len(), 64);
        assert_eq!(event.id.to_hex().len(), 64);
        assert_eq!(event.sig.to_string().len(), 128);
        assert!(event.verify().is_ok());
        assert!(serde_json::to_string(&event.tags)?.contains("libfoo"));
        verify_release_event(&event, &manifest)?;
        let cache_root = tempdir()?;
        cache_release(cache_root.path(), &event)?;
        let locked = LockedPackage {
            publisher: event.pubkey.to_hex(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            sha256: manifest.sha256.clone(),
            dependencies: manifest.dependencies.clone(),
            conflicts: vec![],
            runtime_requires: vec![],
            provides: vec![],
        };
        assert_eq!(
            load_cached_release(cache_root.path(), &locked)?.id,
            event.id
        );
        let mut changed_manifest = manifest.clone();
        changed_manifest.dependencies.clear();
        assert!(verify_release_event(&event, &changed_manifest).is_err());
        let mut malformed = event.clone();
        malformed.tags.push(Tag::parse(vec![
            "depends".to_owned(),
            "malformed".to_owned(),
        ])?);
        assert!(
            manifest_from_release(&malformed, Path::new("artifact"), &manifest.sha256).is_err()
        );
        let mut duplicate = event.clone();
        duplicate
            .tags
            .push(Tag::parse(vec!["version".to_owned(), "9.9.9".to_owned()])?);
        assert!(
            manifest_from_release(&duplicate, Path::new("artifact"), &manifest.sha256).is_err()
        );
        Ok(())
    }

    #[test]
    fn validates_nip34_repository_references() -> Result<()> {
        let publisher = "11".repeat(32);
        validate_repo_reference(&format!("30617:{publisher}:npack"))?;
        assert!(validate_repo_reference("30617:not-a-key:npack").is_err());
        assert!(validate_repo_reference(&format!("30617:{publisher}:../npack")).is_err());
        Ok(())
    }

    #[test]
    fn validates_nip94_artifact_events() -> Result<()> {
        let keys = Keys::parse(&"11".repeat(32))?;
        let manifest = Manifest {
            publisher: keys.public_key().to_hex(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.npk".into(),
            sha256: "00".repeat(32),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        let event = sign_artifact_event(&manifest, "https://blob.example/00", 1, &keys)?;
        validate_artifact_event(&event, &keys.public_key(), &manifest.sha256)?;
        Ok(())
    }

    #[test]
    fn rejects_malformed_release_event_candidates() -> Result<()> {
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.npk".into(),
            sha256: "00".repeat(32),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: Some("artifact-event-id".into()),
            repo: None,
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        let mut event = sign_release_event(&manifest, &"11".repeat(32), 1)?;
        assert!(release_event_is_v1(&event));
        event
            .tags
            .push(Tag::parse(vec!["x".into(), "00".repeat(32)])?);
        assert!(!release_event_is_v1(&event));
        Ok(())
    }

    #[test]
    fn creates_publisher_signed_revocation_event() -> Result<()> {
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.npk".into(),
            sha256: "00".repeat(32),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: Some("artifact-event-id".into()),
            repo: None,
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        let secret = "11".repeat(32);
        let release = sign_release_event(&manifest, &secret, 1_700_000_000)?;
        let revocation = sign_revocation_event(&release, &secret, "security issue", 1_700_000_001)?;
        assert_eq!(revocation.kind.as_u16(), 9901);
        let release_id = release.id.to_hex();
        assert_eq!(tag_value(&revocation, "e"), Some(release_id.as_str()));
        assert!(revocation.verify().is_ok());
        Ok(())
    }

    #[test]
    fn rejects_release_signed_by_the_wrong_publisher() -> Result<()> {
        let keys = Keys::parse(&"11".repeat(32))?;
        let manifest = Manifest {
            publisher: keys.public_key().to_hex(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.npk".into(),
            sha256: "00".repeat(32),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        assert!(sign_release_event(&manifest, &"22".repeat(32), 1_700_000_000).is_err());
        Ok(())
    }

    #[test]
    fn accepts_user_facing_npub_relay_identity() -> Result<()> {
        let keys = Keys::parse(&"11".repeat(32))?;
        let npub = keys.public_key().to_bech32()?;
        assert_eq!(PublicKey::parse(&npub)?, keys.public_key());
        assert_eq!(
            PublicKey::parse(&keys.public_key().to_hex())?,
            keys.public_key()
        );
        let publishers = configured_publishers(vec![npub.clone()], &Config::default())?;
        assert_eq!(publishers, vec![keys.public_key().to_hex()]);
        assert_eq!(
            normalize_publisher_reference(&npub),
            keys.public_key().to_hex()
        );
        assert_eq!(display_publisher(&keys.public_key().to_hex()), npub);
        Ok(())
    }

    #[test]
    fn refuses_install_without_dependencies() -> Result<()> {
        let dir = tempdir()?;
        let artifact = dir.path().join("app.bin");
        fs::write(&artifact, b"app")?;
        let manifest_path = dir.path().join("app.npack.json");
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "app".into(),
            version: "1.0.0".into(),
            artifact: "app.bin".into(),
            sha256: hash_file(&artifact)?,
            dependencies: vec![Dependency {
                publisher: None,
                name: "libfoo".into(),
                requirement: ">=2.0.0".into(),
            }],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: default_format(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        let error =
            install(&manifest, &manifest_path, Some(&dir.path().join("store"))).unwrap_err();
        assert!(error.to_string().contains("missing dependency libfoo"));
        Ok(())
    }

    #[test]
    fn refuses_install_with_conflicting_package() -> Result<()> {
        let dir = tempdir()?;
        let store = dir.path().join("store");
        let artifact = dir.path().join("app.bin");
        fs::write(&artifact, b"app")?;
        fs::create_dir_all(&store)?;
        fs::write(
            store.join("installed.json"),
            serde_json::to_vec(&[InstalledPackage {
                publisher: "pub".into(),
                name: "old".into(),
                version: "1.2.0".into(),
                sha256: "00".repeat(32),
                artifact: store.join("old"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![],
                runtime_requires: vec![],
                provides: vec![],
            }])?,
        )?;
        let manifest = Manifest {
            publisher: "pub".into(),
            name: "new".into(),
            version: "1.0.0".into(),
            artifact: "app.bin".into(),
            sha256: hash_file(&artifact)?,
            dependencies: vec![],
            conflicts: vec![Dependency {
                publisher: Some("pub".into()),
                name: "old".into(),
                requirement: ">=1.0.0".into(),
            }],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "opaque".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        let manifest_path = dir.path().join("manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        let error = install(&manifest, &manifest_path, Some(&store)).unwrap_err();
        assert!(error.to_string().contains("conflicts with installed"));
        Ok(())
    }

    #[test]
    fn packs_and_extracts_npk_archive() -> Result<()> {
        let dir = tempdir()?;
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("bin"))?;
        fs::write(source.join("bin/hello"), b"hello")?;
        let archive = dir.path().join("hello.npk");
        pack_npk(&source, &archive)?;
        let archive_again = dir.path().join("hello-again.npk");
        pack_npk(&source, &archive_again)?;
        assert_eq!(hash_file(&archive)?, hash_file(&archive_again)?);
        let destination = dir.path().join("extracted");
        let files = extract_npk(&archive, &destination)?;
        assert_eq!(files.len(), 2);
        assert_eq!(fs::read(destination.join("bin/hello"))?, b"hello");
        Ok(())
    }

    #[test]
    fn loads_embedded_manifest_from_npk() -> Result<()> {
        let dir = tempdir()?;
        let source = dir.path().join("source");
        fs::create_dir_all(source.join(".npack"))?;
        fs::create_dir_all(source.join("bin"))?;
        fs::write(source.join("bin/hello"), b"hello")?;
        let embedded = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "hello.npk".into(),
            sha256: String::new(),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(
            source.join(".npack/manifest.json"),
            serde_json::to_vec(&embedded)?,
        )?;
        let archive = dir.path().join("hello.npk");
        pack_npk(&source, &archive)?;
        let manifest = load_embedded_manifest(&archive)?;
        assert_eq!(manifest.name, "hello");
        assert_eq!(manifest.sha256, hash_file(&archive)?);
        assert!(
            !npk_entry_paths(&archive)?
                .iter()
                .any(|path| path.starts_with(".npack"))
        );
        Ok(())
    }

    #[test]
    fn rejects_unsafe_embedded_manifest_artifact() -> Result<()> {
        let dir = tempdir()?;
        let source = dir.path().join("source");
        fs::create_dir_all(source.join(".npack"))?;
        let manifest = Manifest {
            publisher: "npub1test".into(),
            name: "hello".into(),
            version: "1.0.0".into(),
            artifact: "../hello.npk".into(),
            sha256: String::new(),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: "npk".into(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        fs::write(
            source.join(".npack/manifest.json"),
            serde_json::to_vec(&manifest)?,
        )?;
        let error = pack_npk(&source, &dir.path().join("hello.npk")).unwrap_err();
        assert!(error.to_string().contains("safe filename"));
        Ok(())
    }

    #[test]
    fn refuses_removing_required_dependency() -> Result<()> {
        let dir = tempdir()?;
        let store = dir.path().join("store");
        fs::create_dir_all(&store)?;
        let installed = vec![
            InstalledPackage {
                publisher: "pub".into(),
                name: "libfoo".into(),
                version: "2.0.0".into(),
                sha256: "00".repeat(32),
                artifact: store.join("libfoo"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![],
                runtime_requires: vec![],
                provides: vec!["libfoo.so.2".into()],
            },
            InstalledPackage {
                publisher: "app-pub".into(),
                name: "app".into(),
                version: "1.0.0".into(),
                sha256: "11".repeat(32),
                artifact: store.join("app"),
                dependencies: vec![Dependency {
                    publisher: Some("pub".into()),
                    name: "libfoo".into(),
                    requirement: ">=2.0.0".into(),
                }],
                conflicts: vec![],
                files: vec![],
                runtime_requires: vec![],
                provides: vec![],
            },
        ];
        fs::write(
            store.join("installed.json"),
            serde_json::to_vec(&installed)?,
        )?;
        let error = remove_package("pub/libfoo", Some(&store)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("required by an installed package")
        );
        Ok(())
    }

    #[test]
    fn refuses_removing_required_runtime_provider() -> Result<()> {
        let dir = tempdir()?;
        let store = dir.path().join("store");
        fs::create_dir_all(&store)?;
        let packages = vec![
            InstalledPackage {
                publisher: "pub".into(),
                name: "libfoo".into(),
                version: "2.0.0".into(),
                sha256: "00".repeat(32),
                artifact: store.join("libfoo"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![],
                runtime_requires: vec![],
                provides: vec!["libfoo.so.2".into()],
            },
            InstalledPackage {
                publisher: "app-pub".into(),
                name: "app".into(),
                version: "1.0.0".into(),
                sha256: "11".repeat(32),
                artifact: store.join("app"),
                dependencies: vec![],
                conflicts: vec![],
                files: vec![],
                runtime_requires: vec!["libfoo.so.2".into()],
                provides: vec![],
            },
        ];
        fs::write(store.join("installed.json"), serde_json::to_vec(&packages)?)?;
        let error = remove_package("pub/libfoo", Some(&store)).unwrap_err();
        assert!(error.to_string().contains("runtime capability"));
        Ok(())
    }

    #[test]
    fn matches_versioned_capabilities() {
        assert!(capability_satisfies(
            "libfoo-api >=2.0.0",
            "libfoo-api@2.4.1"
        ));
        assert!(!capability_satisfies(
            "libfoo-api >=3.0.0",
            "libfoo-api@2.4.1"
        ));
        assert!(capability_satisfies("libfoo.so.2", "libfoo.so.2"));
    }

    #[test]
    fn matches_publisher_qualified_package_requirements() {
        let manifest = Manifest {
            publisher: "pub".into(),
            name: "libfoo".into(),
            version: "2.4.1".into(),
            artifact: "libfoo.npk".into(),
            sha256: "00".repeat(32),
            dependencies: vec![],
            conflicts: vec![],
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: default_format(),
            runtime_requires: vec![],
            provides: vec![],
            post_install: vec![],
        };
        assert!(manifest_satisfies_dependency(
            &manifest,
            &Dependency {
                publisher: Some("pub".into()),
                name: "libfoo".into(),
                requirement: ">=2.0.0".into(),
            }
        ));
        assert!(!manifest_satisfies_dependency(
            &manifest,
            &Dependency {
                publisher: Some("other".into()),
                name: "libfoo".into(),
                requirement: ">=2.0.0".into(),
            }
        ));
    }

    #[test]
    fn includes_host_multiarch_library_paths() {
        let directories = host_library_directories();
        assert!(
            directories
                .iter()
                .any(|directory| directory == Path::new("/lib"))
        );
        if ARCH == "x86_64" {
            assert!(
                directories
                    .iter()
                    .any(|directory| directory == Path::new("/lib/x86_64-linux-gnu"))
            );
        }
    }

    #[test]
    fn post_install_actions_are_scoped() -> Result<()> {
        let dir = tempdir()?;
        run_post_install(
            &[PostInstallAction {
                action: "create-directory".into(),
                path: "var/cache".into(),
            }],
            dir.path(),
            true,
            &[],
        )?;
        assert!(dir.path().join("var/cache").is_dir());
        let error = run_post_install(
            &[PostInstallAction {
                action: "create-directory".into(),
                path: "../escape".into(),
            }],
            dir.path(),
            true,
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsafe post-install path"));
        let error = run_post_install(
            &[PostInstallAction {
                action: "register-service".into(),
                path: "hello.service".into(),
            }],
            dir.path(),
            true,
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("service-manager"));
        Ok(())
    }

    #[test]
    fn rolls_back_post_install_service_and_directories() -> Result<()> {
        let dir = tempdir()?;
        let service = dir.path().join("example.service");
        fs::write(&service, b"new service")?;
        let permissions = fs::metadata(&service)?.permissions();
        let created_directory = dir.path().join("created");
        fs::create_dir(&created_directory)?;

        rollback_post_install(
            std::slice::from_ref(&service),
            std::slice::from_ref(&created_directory),
            &[(
                service.clone(),
                Some((b"old service".to_vec(), permissions)),
            )],
        )?;

        assert_eq!(fs::read(&service)?, b"old service");
        assert!(!created_directory.exists());
        Ok(())
    }

    #[test]
    fn lockfile_validates_dependency_graph_and_conflicts() -> Result<()> {
        let publisher = "11".repeat(32);
        let lockfile = Lockfile {
            version: 1,
            packages: vec![
                LockedPackage {
                    publisher: publisher.clone(),
                    name: "libfoo".into(),
                    version: "2.1.0".into(),
                    sha256: "aa".repeat(32),
                    dependencies: vec![],
                    conflicts: vec![],
                    runtime_requires: vec![],
                    provides: vec![],
                },
                LockedPackage {
                    publisher: "22".repeat(32),
                    name: "app".into(),
                    version: "1.0.0".into(),
                    sha256: "bb".repeat(32),
                    dependencies: vec![Dependency {
                        publisher: Some(publisher.clone()),
                        name: "libfoo".into(),
                        requirement: ">=2.0.0".into(),
                    }],
                    conflicts: vec![],
                    runtime_requires: vec!["libc.so.6".into()],
                    provides: vec![],
                },
            ],
        };
        validate_lockfile_graph(&lockfile)?;

        let mut invalid = lockfile.clone();
        invalid.packages[1].dependencies[0].requirement = ">=3.0.0".into();
        assert!(validate_lockfile_graph(&invalid).is_err());

        let mut capabilities = lockfile.clone();
        capabilities.packages[1]
            .runtime_requires
            .push("libssl.so.3".into());
        let installed = vec![
            format!("{}/{}", publisher, "libfoo"),
            format!("{}/{}", "22".repeat(32), "app"),
        ];
        let root = tempdir()?;
        fs::create_dir_all(root.path().join("packages"))?;
        fs::write(
            root.path().join("installed.json"),
            serde_json::to_vec(&[
                InstalledPackage {
                    publisher: publisher.clone(),
                    name: "libfoo".into(),
                    version: "2.1.0".into(),
                    sha256: "aa".repeat(32),
                    artifact: root.path().join("libfoo"),
                    dependencies: vec![],
                    conflicts: vec![],
                    files: vec![],
                    runtime_requires: vec![],
                    provides: vec![],
                },
                InstalledPackage {
                    publisher: "22".repeat(32),
                    name: "app".into(),
                    version: "1.0.0".into(),
                    sha256: "bb".repeat(32),
                    artifact: root.path().join("app"),
                    dependencies: lockfile.packages[1].dependencies.clone(),
                    conflicts: vec![],
                    files: vec![],
                    runtime_requires: vec![],
                    provides: vec![],
                },
            ])?,
        )?;
        verify_locked_order(&lockfile, &installed)?;
        assert!(verify_locked_install(&capabilities, root.path()).is_err());
        Ok(())
    }
}
