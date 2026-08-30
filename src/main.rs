use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nostr::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(name = "npack", version, about = "A Nostr-native package manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    },
    List {
        #[arg(long)]
        store: Option<PathBuf>,
    },
    ReleaseEvent {
        manifest: PathBuf,
        #[arg(long, help = "32-byte hex-encoded Nostr secret key")]
        secret_key: String,
        #[arg(long, default_value_t = 0)]
        created_at: u64,
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
}

fn default_os() -> String {
    "any".into()
}
fn default_arch() -> String {
    "any".into()
}
fn default_format() -> String {
    "opaque".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Dependency {
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
}

fn main() -> Result<()> {
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
        Command::Install { manifest, store } => {
            let package = load_manifest(&manifest)?;
            verify_manifest(&package, &manifest)?;
            let installed = install(&package, &manifest, store.as_deref())?;
            println!(
                "installed {}/{} {}",
                installed.publisher, installed.name, installed.version
            );
        }
        Command::List { store } => {
            for package in installed_packages(store.as_deref())? {
                println!("{}/{} {}", package.publisher, package.name, package.version);
            }
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
        tags.push(vec![
            "depends".into(),
            dependency.name.clone(),
            dependency.requirement.clone(),
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

fn load_manifest(path: &Path) -> Result<Manifest> {
    let bytes = fs::read(path).with_context(|| format!("reading manifest {}", path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).context("parsing JSON manifest")?;
    if manifest.publisher.is_empty() || manifest.name.is_empty() || manifest.version.is_empty() {
        bail!("manifest publisher, name, and version must not be empty");
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("manifest sha256 must be 64 hexadecimal characters");
    }
    Ok(manifest)
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
    Ok(())
}

fn default_store() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("npack")
}

fn install(
    manifest: &Manifest,
    manifest_path: &Path,
    store: Option<&Path>,
) -> Result<InstalledPackage> {
    let root = store.map(Path::to_path_buf).unwrap_or_else(default_store);
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
    let installed = InstalledPackage {
        publisher: manifest.publisher.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        sha256: manifest.sha256.to_ascii_lowercase(),
        artifact: destination,
    };
    let mut packages = installed_packages(Some(&root))?;
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

fn installed_packages(store: Option<&Path>) -> Result<Vec<InstalledPackage>> {
    let root = store.map(Path::to_path_buf).unwrap_or_else(default_store);
    let path = root.join("installed.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
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
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: default_format(),
        };
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        verify_manifest(&manifest, &manifest_path)?;
        let store = dir.path().join("store");
        let installed = install(&manifest, &manifest_path, Some(&store))?;
        assert_eq!(installed_packages(Some(&store))?.len(), 1);
        assert!(installed.artifact.exists());
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
            artifact_event: None,
            repo: None,
            commit: None,
            os: default_os(),
            arch: default_arch(),
            format: default_format(),
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
                name: "libfoo".into(),
                requirement: ">=2".into(),
            }],
            artifact_event: Some("artifact-event-id".into()),
            repo: Some("30617:publisher:hello".into()),
            commit: Some("commit-sha".into()),
            os: "linux".into(),
            arch: "x86_64".into(),
            format: "tar.zst".into(),
        };
        let event = sign_release_event(&manifest, &"11".repeat(32), 1_700_000_000)?;
        assert_eq!(event.kind.as_u16(), 9900);
        assert_eq!(event.created_at.as_secs(), 1_700_000_000);
        assert_eq!(event.pubkey.to_hex().len(), 64);
        assert_eq!(event.id.to_hex().len(), 64);
        assert_eq!(event.sig.to_string().len(), 128);
        assert!(event.verify().is_ok());
        assert!(serde_json::to_string(&event.tags)?.contains("libfoo"));
        Ok(())
    }
}
