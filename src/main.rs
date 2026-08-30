use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
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
    }
    Ok(())
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
        };
        fs::write(&artifact, b"tampered")?;
        assert!(verify_manifest(&manifest, &dir.path().join("manifest.json")).is_err());
        Ok(())
    }
}
