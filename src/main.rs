use anyhow::{Context, Result, bail};
use bitcoin_hashes::sha256::Hash as Sha256Hash;
use clap::{Parser, Subcommand};
use goblin::Object;
use nostr_blossom::prelude::BlossomClient;
use nostr_sdk::prelude::*;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    env::consts::{ARCH, OS},
    fs, io,
    os::unix::fs::PermissionsExt,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

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

#[derive(Subcommand)]
enum Command {
    Hash {
        artifact: PathBuf,
    },
    Verify {
        manifest: PathBuf,
    },
    Install {
        manifest: PathBuf,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long, help = "Install into the current user's ~/.local prefix")]
        user: bool,
        #[arg(long = "allow-capability")]
        allowed_capabilities: Vec<String>,
    },
    List {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long, help = "List packages installed in the user's prefix")]
        user: bool,
    },
    VerifyInstalled {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long, help = "Verify packages installed in the user's prefix")]
        user: bool,
    },
    ReleaseEvent {
        manifest: PathBuf,
        #[arg(long, help = "32-byte hex-encoded Nostr secret key")]
        secret_key: String,
        #[arg(long, default_value_t = 0)]
        created_at: u64,
    },
    VerifyEvent {
        event: PathBuf,
        manifest: PathBuf,
    },
    RevokeEvent {
        event: PathBuf,
        #[arg(long, help = "32-byte hex-encoded Nostr secret key")]
        secret_key: String,
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
    },
    Fetch {
        sha256: String,
        #[arg(long)]
        server: String,
        #[arg(long)]
        output: PathBuf,
    },
    #[command(alias = "update")]
    InstallRef {
        package: String,
        #[arg(long = "relay")]
        relays: Vec<String>,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long, help = "Install into the current user's ~/.local prefix")]
        user: bool,
        #[arg(long = "trusted-publisher")]
        trusted_publishers: Vec<String>,
        #[arg(long, help = "Nostr pubkey whose NIP-65 relay list should be used")]
        pubkey: Option<String>,
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
        #[arg(long, help = "Remove a package from the user's prefix")]
        user: bool,
    },
    Inspect {
        artifact: PathBuf,
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
        Command::Verify { manifest } => {
            let package = load_manifest(&manifest)?;
            verify_manifest(&package, &manifest)?;
            println!(
                "verified {}/{} {}",
                package.publisher, package.name, package.version
            );
        }
        Command::Install {
            manifest,
            store,
            user,
            allowed_capabilities,
        } => {
            let package = load_manifest(&manifest)?;
            verify_manifest(&package, &manifest)?;
            let installed = install_with_capabilities(
                &package,
                &manifest,
                store.as_deref(),
                user || config.install.user,
                &allowed_capabilities,
            )?;
            println!(
                "installed {}/{} {}",
                installed.publisher, installed.name, installed.version
            );
        }
        Command::List { store, user } => {
            let user = user || config.install.user;
            for package in installed_packages(Some(&install_paths(store.as_deref(), user).0))? {
                println!("{}/{} {}", package.publisher, package.name, package.version);
            }
        }
        Command::VerifyInstalled { store, user } => {
            verify_installed(store.as_deref(), user || config.install.user)?;
        }
        Command::ReleaseEvent {
            manifest,
            secret_key,
            created_at,
        } => {
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
                nostr_event.id, package.publisher, package.name, package.version
            );
        }
        Command::RevokeEvent {
            event,
            secret_key,
            reason,
            created_at,
        } => {
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
        } => {
            let relays = configured_relays(relays, &config)?;
            let trusted_publishers = configured_publishers(trusted_publishers, &config)?;
            let pubkey = pubkey.or_else(|| config.identity.pubkey.clone());
            search_releases(&query, &relays, &trusted_publishers, pubkey.as_deref()).await?
        }
        Command::Fetch {
            sha256,
            server,
            output,
        } => fetch_blob(&sha256, &server, &output).await?,
        Command::InstallRef {
            package,
            relays,
            store,
            user,
            trusted_publishers,
            pubkey,
            allowed_capabilities,
        } => {
            let relays = configured_relays(relays, &config)?;
            let trusted_publishers = configured_publishers(trusted_publishers, &config)?;
            let pubkey = pubkey.or_else(|| config.identity.pubkey.clone());
            let (publisher, name) = package
                .split_once('/')
                .map_or((None, package.as_str()), |(p, n)| {
                    (Some(normalize_publisher_reference(p)), n)
                });
            install_ref(
                &name,
                publisher,
                &relays,
                store.as_deref(),
                user || config.install.user,
                &trusted_publishers,
                pubkey.as_deref(),
                &allowed_capabilities,
            )
            .await?
        }
        Command::Pack { source, output } => pack_npk(&source, &output)?,
        Command::Remove {
            package,
            store,
            user,
        } => remove_package_at(&package, store.as_deref(), user || config.install.user)?,
        Command::Inspect { artifact } => inspect_artifact(&artifact)?,
    }
    Ok(())
}

fn inspect_artifact(path: &Path) -> Result<()> {
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

async fn fetch_blob(sha256: &str, server: &str, output: &Path) -> Result<()> {
    let expected: Sha256Hash = sha256
        .parse()
        .context("sha256 must be 64 hexadecimal characters")?;
    let client = BlossomClient::new(Url::parse(server)?);
    let bytes = client.get_blob::<Keys>(expected, None, None, None).await?;
    let actual = Sha256Hash::hash(&bytes);
    if actual != expected {
        bail!("Blossom response hash mismatch: expected {expected}, got {actual}");
    }
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

async fn install_ref(
    name: &str,
    publisher: Option<String>,
    relays: &[String],
    store: Option<&Path>,
    user: bool,
    trusted_publishers: &[String],
    user_pubkey: Option<&str>,
    allowed_capabilities: &[String],
) -> Result<()> {
    let client = Client::default();
    for relay in relays {
        client.add_relay(relay).await?;
    }
    client.connect().await;
    add_user_relays(&client, user_pubkey).await?;
    let (root, prefix) = install_paths(store, user);
    let mut visiting = Vec::new();
    let mut installed = Vec::new();
    install_remote_package(
        &client,
        name,
        publisher,
        None,
        allowed_capabilities,
        user,
        trusted_publishers,
        user_pubkey,
        &root,
        &prefix,
        &mut visiting,
        &mut installed,
    )
    .await?;
    client.disconnect().await;
    println!("install order: {}", installed.join(" -> "));
    Ok(())
}

use std::future::Future;
use std::pin::Pin;

fn install_remote_package<'a>(
    client: &'a Client,
    name: &'a str,
    publisher: Option<String>,
    requirement: Option<String>,
    allowed_capabilities: &'a [String],
    user: bool,
    trusted_publishers: &'a [String],
    user_pubkey: Option<&'a str>,
    root: &'a Path,
    prefix: &'a Path,
    visiting: &'a mut Vec<String>,
    installed: &'a mut Vec<String>,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let install_key = publisher.as_deref().map_or_else(
            || name.to_owned(),
            |publisher| format!("{publisher}/{name}"),
        );
        if installed
            .iter()
            .any(|installed_name| installed_name == &install_key)
        {
            return Ok(());
        }
        if !trusted_publishers.is_empty()
            && publisher.as_deref().map_or(false, |publisher| {
                !trusted_publishers
                    .iter()
                    .any(|trusted| trusted == publisher)
            })
        {
            bail!("publisher is not in the trusted publisher list");
        }
        if visiting
            .iter()
            .any(|visiting_name| visiting_name == &install_key)
        {
            bail!(
                "dependency cycle detected: {} -> {}",
                visiting.join(" -> "),
                name
            );
        }
        visiting.push(install_key.clone());
        let releases = client
            .fetch_events(Filter::new().kind(Kind::Custom(9900)).limit(500))
            .timeout(std::time::Duration::from_secs(10))
            .await?;
        let revoked = client
            .fetch_events(Filter::new().kind(Kind::Custom(9901)).limit(500))
            .timeout(std::time::Duration::from_secs(10))
            .await?
            .into_iter()
            .filter(|event| event.verify().is_ok())
            .filter_map(|event| {
                tag_value(&event, "e").map(|release| (event.pubkey.to_hex(), release.to_owned()))
            })
            .collect::<Vec<_>>();
        let release = releases
            .into_iter()
            .filter(|event| event.verify().is_ok())
            .filter(|event| {
                trusted_publishers.is_empty()
                    || trusted_publishers
                        .iter()
                        .any(|trusted| trusted == &event.pubkey.to_hex())
            })
            .filter(|event| {
                !revoked.iter().any(|(publisher, release_id)| {
                    publisher == &event.pubkey.to_hex() && release_id == &event.id.to_hex()
                })
            })
            .filter(|event| tag_value(event, "name") == Some(name))
            .filter(release_matches_host)
            .filter(|event| {
                publisher
                    .as_deref()
                    .map_or(true, |publisher| event.pubkey.to_hex() == publisher)
            })
            .filter(|event| {
                requirement.as_deref().map_or(true, |req| {
                    VersionReq::parse(req)
                        .ok()
                        .zip(tag_value(event, "version").and_then(|v| Version::parse(v).ok()))
                        .map(|(req, version)| req.matches(&version))
                        .unwrap_or(false)
                })
            })
            .max_by_key(|event| {
                tag_value(event, "version")
                    .and_then(|v| Version::parse(v).ok())
                    .unwrap_or_else(|| Version::new(0, 0, 0))
            })
            .with_context(|| format!("no verified release found for {name}"))?;
        let artifact_event_id = tag_value(&release, "artifact")
            .context("release has no artifact event")?
            .parse::<EventId>()?;
        let artifact_events = client
            .fetch_events(Filter::new().kind(Kind::Custom(1063)).id(artifact_event_id))
            .timeout(std::time::Duration::from_secs(10))
            .await?;
        let artifact_event = artifact_events
            .into_iter()
            .find(|event| event.verify().is_ok())
            .context("no verified NIP-94 artifact event found")?;
        if artifact_event.pubkey != release.pubkey {
            bail!("release and artifact event publishers do not match");
        }
        let sha256 = tag_value(&release, "x").context("release has no artifact hash")?;
        if tag_value(&artifact_event, "x") != Some(sha256) {
            bail!("release and artifact event hashes do not match");
        }
        let expected: Sha256Hash = sha256.parse()?;
        let urls = artifact_event
            .tags
            .iter()
            .filter(|tag| tag.kind() == "url")
            .filter_map(Tag::content)
            .collect::<Vec<_>>();
        if urls.is_empty() {
            bail!("artifact event has no URL");
        }
        let mut bytes = None;
        for url in urls {
            let mut server_url = match Url::parse(url) {
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
        let bytes = bytes.context("no artifact mirror returned the expected SHA-256")?;
        let staging = root.join("downloads").join(sha256);
        fs::create_dir_all(&staging)?;
        let artifact_path = staging.join("artifact");
        fs::write(&artifact_path, bytes)?;
        let manifest = manifest_from_release(&release, &artifact_path, sha256)?;
        let dependencies = manifest.dependencies.clone();
        for dependency in dependencies {
            install_remote_package(
                client,
                &dependency.name,
                dependency.publisher,
                Some(dependency.requirement),
                allowed_capabilities,
                user,
                trusted_publishers,
                user_pubkey,
                root,
                prefix,
                visiting,
                installed,
            )
            .await?;
        }
        install_with_capabilities_at(
            &manifest,
            &staging.join("manifest.json"),
            Some(&root),
            Some(prefix),
            user,
            allowed_capabilities,
        )?;
        visiting.pop();
        installed.push(install_key);
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
        if !is_write_only {
            if let Some(relay) = values.get(1) {
                client.add_relay(relay).await?;
            }
        }
    }
    client.connect().await;
    Ok(())
}

fn manifest_from_release(event: &Event, artifact: &Path, sha256: &str) -> Result<Manifest> {
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
        .collect();
    let provides = event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "provides")
        .filter_map(Tag::content)
        .map(str::to_owned)
        .collect();
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
        repo: tag_value(event, "repo").map(str::to_owned),
        commit: tag_value(event, "commit").map(str::to_owned),
        os: tag_value(event, "os").unwrap_or("any").into(),
        arch: tag_value(event, "arch").unwrap_or("any").into(),
        format: tag_value(event, "format").unwrap_or("opaque").into(),
        runtime_requires,
        provides,
        post_install,
    })
}

async fn search_releases(
    query: &str,
    relays: &[String],
    trusted_publishers: &[String],
    user_pubkey: Option<&str>,
) -> Result<()> {
    let client = Client::default();
    for relay in relays {
        client
            .add_relay(relay)
            .await
            .with_context(|| format!("adding relay {relay}"))?;
    }
    client.connect().await;
    add_user_relays(&client, user_pubkey).await?;
    let filter = Filter::new().kind(Kind::Custom(9900)).limit(500);
    let events = client
        .fetch_events(filter)
        .timeout(std::time::Duration::from_secs(10))
        .await
        .context("querying Nostr relays")?;
    let revoked = client
        .fetch_events(Filter::new().kind(Kind::Custom(9901)).limit(500))
        .timeout(std::time::Duration::from_secs(10))
        .await?
        .into_iter()
        .filter(|event| event.verify().is_ok())
        .filter_map(|event| {
            tag_value(&event, "e").map(|release| (event.pubkey.to_hex(), release.to_owned()))
        })
        .collect::<Vec<_>>();
    let query = query.to_ascii_lowercase();
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
        let matches = event.tags.iter().any(|tag| {
            tag.kind() == "name"
                && tag
                    .content()
                    .map(|name| name.to_ascii_lowercase().contains(&query))
                    .unwrap_or(false)
        });
        if matches && event.verify().is_ok() {
            println!("{} {} {}", event.id, event.pubkey, event.content);
        }
    }
    client.disconnect().await;
    Ok(())
}

fn verify_release_event(event: &Event, manifest: &Manifest) -> Result<()> {
    if event.kind.as_u16() != 9900 {
        bail!("expected npack release event kind 9900, got {}", event.kind);
    }
    event
        .verify()
        .context("invalid Nostr event ID or signature")?;
    for (kind, expected) in [
        ("name", manifest.name.as_str()),
        ("version", manifest.version.as_str()),
        ("os", manifest.os.as_str()),
        ("arch", manifest.arch.as_str()),
        ("format", manifest.format.as_str()),
        ("x", manifest.sha256.to_ascii_lowercase().as_str()),
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
    Ok(())
}

fn sign_release_event(manifest: &Manifest, secret_hex: &str, created_at: u64) -> Result<Event> {
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    let mut tags = vec![
        vec![
            "d".into(),
            format!("{}/{}/{}", manifest.name, manifest.version, manifest.arch),
        ],
        vec!["name".into(), manifest.name.clone()],
        vec!["version".into(), manifest.version.clone()],
        vec!["os".into(), manifest.os.clone()],
        vec!["arch".into(), manifest.arch.clone()],
        vec!["format".into(), manifest.format.clone()],
        vec!["x".into(), manifest.sha256.to_ascii_lowercase()],
    ];
    if let Some(event) = &manifest.artifact_event {
        tags.push(vec!["artifact".into(), event.clone()]);
    }
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
    EventBuilder::new(Kind::Custom(9900), content)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys)
        .map_err(Into::into)
}

fn sign_revocation_event(
    release: &Event,
    secret_hex: &str,
    reason: &str,
    created_at: u64,
) -> Result<Event> {
    if release.kind.as_u16() != 9900 {
        bail!("can only revoke a kind:9900 release event");
    }
    release
        .verify()
        .context("release event has invalid signature")?;
    let keys = Keys::parse(secret_hex).context("secret key must be hex or nsec")?;
    if keys.public_key() != release.pubkey {
        bail!("revocation signer must match the release publisher");
    }
    let tags = vec![
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
    EventBuilder::new(Kind::Custom(9901), reason)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .finalize(&keys)
        .map_err(Into::into)
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let bytes = fs::read(path).with_context(|| format!("reading manifest {}", path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).context("parsing JSON manifest")?;
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
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("manifest sha256 must be 64 hexadecimal characters");
    }
    validate_dependency_declarations(&manifest.dependencies)?;
    validate_dependency_declarations(&manifest.conflicts)?;
    Ok(manifest)
}

fn validate_dependency_declarations(dependencies: &[Dependency]) -> Result<()> {
    for dependency in dependencies {
        if dependency.name.is_empty()
            || dependency.name.contains('/')
            || dependency.name.contains('\\')
            || dependency.name == "."
            || dependency.name == ".."
        {
            bail!("dependency name must be a single safe path component");
        }
        if let Some(publisher) = &dependency.publisher {
            if publisher.is_empty()
                || publisher.contains('/')
                || publisher.contains('\\')
                || publisher == "."
                || publisher == ".."
            {
                bail!("dependency publisher must be a single safe path component");
            }
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
    if relays.is_empty() {
        bail!("no relays configured; pass --relay or configure [network].relays");
    }
    Ok(relays)
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
        install_staged_npk(&staging, &prefix, &npk_entry_paths(&destination)?)?
    } else {
        vec![destination.clone()]
    };
    let mut files = files;
    files.extend(run_post_install(
        &manifest.post_install,
        &prefix,
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
        files,
        runtime_requires: manifest.runtime_requires.clone(),
        provides: manifest.provides.clone(),
    };
    remove_stale_files(&existing_packages, &installed)?;
    let mut packages = existing_packages;
    packages.retain(|p| {
        !(p.publisher == installed.publisher
            && p.name == installed.name
            && p.version == installed.version)
    });
    packages.push(installed.clone());
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("installed.json"),
        serde_json::to_vec_pretty(&packages)?,
    )?;
    Ok(installed)
}

fn remove_stale_files(existing: &[InstalledPackage], replacement: &InstalledPackage) -> Result<()> {
    let previous = existing.iter().find(|package| {
        package.publisher == replacement.publisher
            && package.name == replacement.name
            && package.version == replacement.version
    });
    let Some(previous) = previous else {
        return Ok(());
    };
    for file in &previous.files {
        if replacement
            .files
            .iter()
            .any(|replacement_file| replacement_file == file)
        {
            continue;
        }
        let owned_by_other_package = existing.iter().any(|package| {
            !(package.publisher == previous.publisher
                && package.name == previous.name
                && package.version == previous.version)
                && package.files.iter().any(|other_file| other_file == file)
        });
        if owned_by_other_package {
            continue;
        }
        if file.is_file()
            || fs::symlink_metadata(file).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::remove_file(file)?;
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
    for action in actions {
        match action.action.as_str() {
            "create-directory" => fs::create_dir_all(prefix.join(&action.path))?,
            "register-service" => {
                let source = prefix.join(&action.path);
                if source.extension().and_then(|extension| extension.to_str()) != Some("service") {
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
                fs::copy(source, &destination)?;
                created.push(destination);
            }
            _ => bail!("unsupported post-install action: {}", action.action),
        }
    }
    Ok(created)
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
    let backup_root = staging.join(".npack-backups");
    for relative in entries {
        let result = (|| -> Result<()> {
            let source = staging.join(relative);
            let destination = prefix.join(relative);
            let metadata = fs::symlink_metadata(&source)?;
            if metadata.file_type().is_dir() {
                fs::create_dir_all(&destination)?;
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
            return Err(error);
        }
    }
    Ok(installed)
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
        if let Some(backup) = backup {
            if backup.exists() || fs::symlink_metadata(backup).is_ok() {
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
    }
    Ok(())
}

fn ensure_install_paths_available(
    paths: &[PathBuf],
    installed: &[InstalledPackage],
    publisher: &str,
    name: &str,
    version: &str,
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
                && package.version == version
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
                    .map_or(true, |publisher| package.publisher == publisher)
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
                    .map_or(true, |publisher| package.publisher == publisher)
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
        }) && system
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
    for directory in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("lib") && name.contains(".so") {
                    capabilities.insert(name);
                }
            }
        }
    }
    capabilities
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
                package.publisher, package.name, package.version
            )
        })?;
        if actual != package.sha256.to_ascii_lowercase() {
            bail!(
                "installed artifact hash mismatch for {}/{} {}",
                package.publisher,
                package.name,
                package.version
            );
        }
        for file in &package.files {
            if fs::symlink_metadata(file).is_err() {
                bail!(
                    "installed file is missing for {}/{} {}: {}",
                    package.publisher,
                    package.name,
                    package.version,
                    file.display()
                );
            }
        }
        println!(
            "verified {}/{} {}",
            package.publisher, package.name, package.version
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
                    .map_or(true, |dependency_publisher| {
                        dependency_publisher == publisher
                    })
        })
    }) {
        bail!("cannot remove {package}: it is required by an installed package");
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
                if file.is_file()
                    || fs::symlink_metadata(file)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    if file.exists() || fs::symlink_metadata(file).is_ok() {
                        fs::remove_file(file)?;
                    }
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
            &[PathBuf::from("bin/one"), PathBuf::from("bin/missing")],
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("No such file") || error.to_string().contains("not found")
        );
        assert!(!prefix.join("bin/one").exists());
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
            repo: Some("30617:publisher:hello".into()),
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
        fs::create_dir_all(&destination)?;
        let files = extract_npk(&archive, &destination)?;
        assert_eq!(files.len(), 2);
        assert_eq!(fs::read(destination.join("bin/hello"))?, b"hello");
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
}
