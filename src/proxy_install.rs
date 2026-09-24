//! Installs only the native proxy. Backends remain user-managed executables.

use std::{fs, path::Path};
use zed_extension_api::{self as zed, Architecture, LanguageServerId, Os, Result};

use crate::BASH_PROXY;

const REPOSITORY: &str = "tzabbi/zed-gitlab-ci-ls";
// Release assets are built from the extension's tag, not the proxy crate's version.
const RELEASE_TAG: &str = concat!("v", env!("CARGO_PKG_VERSION"));

pub fn binary_path(
    id: &LanguageServerId,
    worktree: &zed::Worktree,
    configured: Option<String>,
) -> Result<String> {
    resolve(configured, Path::new("."), &ZedHost { id, worktree })
}

trait Host {
    fn which(&self) -> Option<String>;
    fn platform(&self) -> (Os, Architecture);
    fn release(&self) -> Result<zed::GithubRelease>;
    fn download(&self, url: &str, path: &str) -> Result<()>;
    fn make_executable(&self, path: &str) -> Result<()>;
    fn status(&self, status: zed::LanguageServerInstallationStatus);
}

struct ZedHost<'a> {
    id: &'a LanguageServerId,
    worktree: &'a zed::Worktree,
}

impl Host for ZedHost<'_> {
    fn which(&self) -> Option<String> {
        self.worktree.which(BASH_PROXY)
    }

    fn platform(&self) -> (Os, Architecture) {
        zed::current_platform()
    }

    fn release(&self) -> Result<zed::GithubRelease> {
        zed::github_release_by_tag_name(REPOSITORY, RELEASE_TAG)
    }

    fn download(&self, url: &str, path: &str) -> Result<()> {
        zed::download_file(url, path, zed::DownloadedFileType::Uncompressed)
    }

    fn make_executable(&self, path: &str) -> Result<()> {
        zed::make_file_executable(path)
    }

    fn status(&self, status: zed::LanguageServerInstallationStatus) {
        zed::set_language_server_installation_status(self.id, &status);
    }
}

struct PlatformAsset {
    target: &'static str,
    executable: &'static str,
}

impl PlatformAsset {
    fn new(os: Os, architecture: Architecture) -> Result<Self> {
        let target = match (os, architecture) {
            (Os::Linux, Architecture::X8664) => "x86_64-unknown-linux-musl",
            (Os::Linux, Architecture::Aarch64) => "aarch64-unknown-linux-musl",
            (Os::Mac, Architecture::X8664) => "x86_64-apple-darwin",
            (Os::Mac, Architecture::Aarch64) => "aarch64-apple-darwin",
            (Os::Windows, Architecture::X8664) => "x86_64-pc-windows-msvc",
            _ => return Err(format!("no prebuilt proxy for {os:?}/{architecture:?}")),
        };
        Ok(Self {
            target,
            executable: if os == Os::Windows {
                "gitlab-ci-bash-ls.exe"
            } else {
                BASH_PROXY
            },
        })
    }

    fn name(&self) -> String {
        let suffix = if self.executable.ends_with(".exe") {
            ".exe"
        } else {
            ""
        };
        format!("{BASH_PROXY}-{}{suffix}", self.target)
    }
}

fn resolve(configured: Option<String>, root: &Path, host: &impl Host) -> Result<String> {
    let result = configured
        .or_else(|| host.which())
        .map(Ok)
        .unwrap_or_else(|| {
            install(root, host).map_err(|error| {
                format!(
                    "Could not install {BASH_PROXY} from {REPOSITORY} ({RELEASE_TAG}): {error}. \
                     Check your network access and that the release contains the matching asset. \
                     Alternatively, run `cargo install --locked --path {BASH_PROXY}` from a checkout, \
                     set `lsp.{BASH_PROXY}.binary.path`, or disable the proxy with \
                     `languages.\"Gitlab-CI\".language_servers: [\"gitlab-ci\", \"!{BASH_PROXY}\"]`."
                )
            })
        });
    match &result {
        Ok(_) => host.status(zed::LanguageServerInstallationStatus::None),
        Err(error) => host.status(zed::LanguageServerInstallationStatus::Failed(error.clone())),
    }
    result
}

fn install(root: &Path, host: &impl Host) -> Result<String> {
    let (os, architecture) = host.platform();
    let asset = PlatformAsset::new(os, architecture)?;
    let directory = root.join(BASH_PROXY).join(RELEASE_TAG).join(asset.target);
    let binary = directory.join(asset.executable);
    let path = binary.to_str().ok_or("non-UTF-8 installation path")?;
    // A final filename is only created after a completed download and chmod.
    // Check it before contacting GitHub, so restarts continue working offline.
    if nonempty_file(&binary)? {
        host.make_executable(path)?;
        return Ok(path.to_owned());
    }

    host.status(zed::LanguageServerInstallationStatus::CheckingForUpdate);
    let release = host.release()?;
    if release.version != RELEASE_TAG {
        return Err(format!(
            "expected release {RELEASE_TAG}, got {}",
            release.version
        ));
    }
    let name = asset.name();
    let download = release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("release asset `{name}` is missing"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let staging = directory.join(format!("{}.download", asset.executable));
    let staging_path = staging.to_str().ok_or("non-UTF-8 download path")?;
    remove_if_present(&staging)?;
    host.status(zed::LanguageServerInstallationStatus::Downloading);
    let result = (|| {
        host.download(&download.download_url, staging_path)?;
        if !nonempty_file(&staging)? {
            return Err("download did not produce a nonempty executable".to_owned());
        }
        host.make_executable(staging_path)?;
        // Replace an invalid empty cache file too (rename cannot overwrite on Windows).
        remove_if_present(&binary)?;
        fs::rename(&staging, &binary)
            .map_err(|error| format!("cannot finish installation to {path}: {error}"))?;
        Ok(path.to_owned())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn nonempty_file(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && metadata.len() > 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "gitlab-ci-proxy-install-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn binary(&self, tag: &str, target: &str, executable: &str) -> PathBuf {
            self.0
                .join(BASH_PROXY)
                .join(tag)
                .join(target)
                .join(executable)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakeHost {
        on_path: Option<String>,
        platform: (Os, Architecture),
        release: Result<zed::GithubRelease>,
        content: &'static [u8],
        download_error: bool,
        chmod_error: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeHost {
        fn new(os: Os, arch: Architecture) -> Self {
            let assets = PlatformAsset::new(os, arch)
                .ok()
                .map(|asset| zed::GithubReleaseAsset {
                    name: asset.name(),
                    download_url: "https://example.invalid/proxy".to_owned(),
                })
                .into_iter()
                .collect();
            Self {
                on_path: None,
                platform: (os, arch),
                release: Ok(zed::GithubRelease {
                    version: RELEASE_TAG.to_owned(),
                    assets,
                }),
                content: b"complete binary",
                download_error: false,
                chmod_error: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn called(&self, prefix: &str) -> bool {
            self.calls
                .borrow()
                .iter()
                .any(|call| call.starts_with(prefix))
        }
    }

    impl Host for FakeHost {
        fn which(&self) -> Option<String> {
            self.calls.borrow_mut().push("which".to_owned());
            self.on_path.clone()
        }
        fn platform(&self) -> (Os, Architecture) {
            self.calls.borrow_mut().push("platform".to_owned());
            self.platform
        }
        fn release(&self) -> Result<zed::GithubRelease> {
            self.calls
                .borrow_mut()
                .push(format!("release:{REPOSITORY}/{RELEASE_TAG}"));
            self.release.clone()
        }
        fn download(&self, url: &str, path: &str) -> Result<()> {
            self.calls.borrow_mut().push(format!("download:{url}"));
            fs::write(path, self.content).unwrap();
            if self.download_error {
                Err("connection interrupted".to_owned())
            } else {
                Ok(())
            }
        }
        fn make_executable(&self, path: &str) -> Result<()> {
            self.calls.borrow_mut().push(format!("chmod:{path}"));
            if self.chmod_error {
                Err("permission denied".to_owned())
            } else {
                Ok(())
            }
        }
        fn status(&self, status: zed::LanguageServerInstallationStatus) {
            let status = match status {
                zed::LanguageServerInstallationStatus::None => "None".to_owned(),
                zed::LanguageServerInstallationStatus::CheckingForUpdate => {
                    "CheckingForUpdate".to_owned()
                }
                zed::LanguageServerInstallationStatus::Downloading => "Downloading".to_owned(),
                zed::LanguageServerInstallationStatus::Failed(error) => format!("Failed: {error}"),
            };
            self.calls.borrow_mut().push(format!("status:{status}"));
        }
    }

    #[test]
    fn matches_published_asset_names() {
        for (os, arch, expected) in [
            (
                Os::Linux,
                Architecture::X8664,
                "gitlab-ci-bash-ls-x86_64-unknown-linux-musl",
            ),
            (
                Os::Linux,
                Architecture::Aarch64,
                "gitlab-ci-bash-ls-aarch64-unknown-linux-musl",
            ),
            (
                Os::Mac,
                Architecture::X8664,
                "gitlab-ci-bash-ls-x86_64-apple-darwin",
            ),
            (
                Os::Mac,
                Architecture::Aarch64,
                "gitlab-ci-bash-ls-aarch64-apple-darwin",
            ),
            (
                Os::Windows,
                Architecture::X8664,
                "gitlab-ci-bash-ls-x86_64-pc-windows-msvc.exe",
            ),
        ] {
            assert_eq!(PlatformAsset::new(os, arch).unwrap().name(), expected);
            assert!(include_str!("../.github/workflows/release.yaml").contains(expected));
        }
    }

    #[test]
    fn honors_overrides_then_path_even_on_unsupported_platforms() {
        let root = TempDir::new();
        let mut host = FakeHost::new(Os::Windows, Architecture::Aarch64);
        host.on_path = Some("local-proxy.exe".to_owned());
        assert_eq!(
            resolve(Some("explicit-proxy.exe".to_owned()), &root.0, &host).unwrap(),
            "explicit-proxy.exe"
        );
        assert!(!host.called("which"));
        assert!(!host.called("platform"));
        assert_eq!(resolve(None, &root.0, &host).unwrap(), "local-proxy.exe");
        assert!(!host.called("platform"));
        assert!(!host.called("release:"));
    }

    #[test]
    fn completed_cache_is_reused_without_network() {
        let root = TempDir::new();
        let mut host = FakeHost::new(Os::Linux, Architecture::X8664);
        let path = root.binary(RELEASE_TAG, "x86_64-unknown-linux-musl", BASH_PROXY);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"cached executable").unwrap();
        host.release = Err("offline".to_owned());
        assert_eq!(Path::new(&resolve(None, &root.0, &host).unwrap()), path);
        assert!(!host.called("release:"));
        assert!(!host.called("download:"));
        assert!(host.called("chmod:"));
    }

    #[test]
    fn path_wins_over_cache() {
        let root = TempDir::new();
        let mut host = FakeHost::new(Os::Linux, Architecture::X8664);
        let cached = resolve(None, &root.0, &host).unwrap();
        assert!(Path::new(&cached).is_file());
        host.calls.borrow_mut().clear();
        host.on_path = Some("user-proxy".to_owned());
        assert_eq!(resolve(None, &root.0, &host).unwrap(), "user-proxy");
        assert!(!host.called("platform"));
    }

    #[test]
    fn downloads_exact_version_and_publishes_only_finished_files() {
        for (os, arch, target, executable) in [
            (
                Os::Linux,
                Architecture::X8664,
                "x86_64-unknown-linux-musl",
                BASH_PROXY,
            ),
            (
                Os::Windows,
                Architecture::X8664,
                "x86_64-pc-windows-msvc",
                "gitlab-ci-bash-ls.exe",
            ),
        ] {
            let root = TempDir::new();
            let host = FakeHost::new(os, arch);
            let installed = resolve(None, &root.0, &host).unwrap();
            let expected = root.binary(RELEASE_TAG, target, executable);
            assert_eq!(Path::new(&installed), expected);
            assert_eq!(fs::read(&installed).unwrap(), b"complete binary");
            assert!(
                !expected
                    .with_file_name(format!("{executable}.download"))
                    .exists()
            );
            let calls = host.calls.borrow();
            assert!(
                calls
                    .iter()
                    .any(|call| call == &format!("release:{REPOSITORY}/{RELEASE_TAG}"))
            );
            let download = calls
                .iter()
                .position(|call| call.starts_with("download:"))
                .unwrap();
            let chmod = calls
                .iter()
                .position(|call| call.starts_with("chmod:"))
                .unwrap();
            assert!(download < chmod);
            assert!(calls[chmod].ends_with(".download"));
            assert_eq!(calls.last().unwrap(), "status:None");
        }
    }

    #[test]
    fn interrupted_empty_and_non_executable_downloads_never_enter_cache() {
        for failure in ["interrupted", "empty", "chmod"] {
            let root = TempDir::new();
            let mut host = FakeHost::new(Os::Linux, Architecture::X8664);
            match failure {
                "interrupted" => host.download_error = true,
                "empty" => host.content = b"",
                "chmod" => host.chmod_error = true,
                _ => unreachable!(),
            }
            assert!(resolve(None, &root.0, &host).is_err());
            let expected = root.binary(RELEASE_TAG, "x86_64-unknown-linux-musl", BASH_PROXY);
            assert!(!expected.exists());
            assert!(
                !expected
                    .with_file_name(format!("{BASH_PROXY}.download"))
                    .exists()
            );
            assert!(host.called("status:Failed"));
            host.download_error = false;
            host.chmod_error = false;
            host.content = b"retried binary";
            assert_eq!(
                fs::read(resolve(None, &root.0, &host).unwrap()).unwrap(),
                b"retried binary"
            );
        }
    }

    #[test]
    fn empty_cache_and_stale_partial_download_are_replaced() {
        let root = TempDir::new();
        let host = FakeHost::new(Os::Linux, Architecture::X8664);
        let expected = root.binary(RELEASE_TAG, "x86_64-unknown-linux-musl", BASH_PROXY);
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, b"").unwrap();
        fs::write(
            expected.with_file_name(format!("{BASH_PROXY}.download")),
            b"stale data",
        )
        .unwrap();
        let path = resolve(None, &root.0, &host).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"complete binary");
        assert!(host.called("download:"));
    }

    #[test]
    fn mismatched_caches_are_not_used_or_deleted_when_offline() {
        let root = TempDir::new();
        let mut host = FakeHost::new(Os::Linux, Architecture::X8664);
        for (tag, target) in [
            ("v0.0.0", "x86_64-unknown-linux-musl"),
            (RELEASE_TAG, "aarch64-unknown-linux-musl"),
        ] {
            let old = root.binary(tag, target, BASH_PROXY);
            fs::create_dir_all(old.parent().unwrap()).unwrap();
            fs::write(&old, b"other executable").unwrap();
            host.release = Err("offline".to_owned());
            let error = resolve(None, &root.0, &host).unwrap_err();
            assert!(error.contains("offline"));
            assert!(error.contains(RELEASE_TAG));
            assert!(error.contains("binary.path"));
            assert!(old.is_file());
        }
    }

    #[test]
    fn reports_missing_assets_wrong_tags_and_unsupported_platforms() {
        let root = TempDir::new();
        let mut host = FakeHost::new(Os::Linux, Architecture::X8664);
        host.release.as_mut().unwrap().assets.clear();
        assert!(
            resolve(None, &root.0, &host)
                .unwrap_err()
                .contains("gitlab-ci-bash-ls-x86_64-unknown-linux-musl")
        );
        host.release.as_mut().unwrap().version = "v0.0.0".to_owned();
        assert!(
            resolve(None, &root.0, &host)
                .unwrap_err()
                .contains("expected release")
        );
        assert!(!host.called("download:"));
        for (os, arch) in [
            (Os::Windows, Architecture::Aarch64),
            (Os::Linux, Architecture::X86),
        ] {
            let host = FakeHost::new(os, arch);
            assert!(
                resolve(None, &root.0, &host)
                    .unwrap_err()
                    .contains("no prebuilt proxy")
            );
            assert!(!host.called("release:"));
        }
    }
}
