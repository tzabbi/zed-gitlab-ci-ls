use std::{env, fs, path::Path};

use zed_extension_api::{
    self as zed, Architecture, DownloadedFileType, GithubReleaseOptions, LanguageServerId,
    LanguageServerInstallationStatus, Os, Result,
};

const REPOSITORY: &str = "koalaman/shellcheck";
const DIR_PREFIX: &str = "shellcheck-";

/// Returns the ShellCheck binary on the PATH or downloads the latest release.
pub fn binary_path(
    cached: &mut Option<String>,
    language_server_id: &LanguageServerId,
    worktree: &zed::Worktree,
) -> Result<String> {
    if let Some(path) = worktree.which("shellcheck") {
        return Ok(path);
    }

    if let Some(path) = cached.as_ref().filter(|path| is_file(path)) {
        return Ok(path.clone());
    }

    // Without network access an earlier download is still good enough.
    let installed = install(language_server_id).or_else(|err| downloaded().ok_or(err));
    zed::set_language_server_installation_status(
        language_server_id,
        &LanguageServerInstallationStatus::None,
    );

    let path = absolute(&installed?)?;
    *cached = Some(path.clone());
    Ok(path)
}

fn install(language_server_id: &LanguageServerId) -> Result<String> {
    zed::set_language_server_installation_status(
        language_server_id,
        &LanguageServerInstallationStatus::CheckingForUpdate,
    );

    let release = zed::latest_github_release(
        REPOSITORY,
        GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )?;

    let version = &release.version;
    let dir = format!("{DIR_PREFIX}{version}");
    let (asset_name, file_type) = asset(version)?;
    let path = binary_in(&dir);

    if !is_file(&path) {
        zed::set_language_server_installation_status(
            language_server_id,
            &LanguageServerInstallationStatus::Downloading,
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("ShellCheck {version} has no asset {asset_name}"))?;

        zed::download_file(&asset.download_url, &dir, file_type)
            .map_err(|err| format!("failed to download {asset_name}: {err}"))?;
        zed::make_file_executable(&path)?;

        remove_other_versions(&dir);
    }

    Ok(path)
}

fn asset(version: &str) -> Result<(String, DownloadedFileType)> {
    let (os, arch) = zed::current_platform();

    let os = match os {
        Os::Windows => return Ok((format!("shellcheck-{version}.zip"), DownloadedFileType::Zip)),
        Os::Mac => "darwin",
        Os::Linux => "linux",
    };
    let arch = match arch {
        Architecture::Aarch64 => "aarch64",
        Architecture::X8664 => "x86_64",
        Architecture::X86 => return Err("ShellCheck has no release for x86".to_string()),
    };

    Ok((
        format!("shellcheck-{version}.{os}.{arch}.tar.gz"),
        DownloadedFileType::GzipTar,
    ))
}

/// The tarballs unpack into a folder named like the release, the Windows zip
/// has the binary at its root.
fn binary_in(dir: &str) -> String {
    match zed::current_platform().0 {
        Os::Windows => format!("{dir}/shellcheck.exe"),
        Os::Mac | Os::Linux => format!("{dir}/{dir}/shellcheck"),
    }
}

fn downloaded() -> Option<String> {
    fs::read_dir(".")
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(DIR_PREFIX))
        .map(|name| binary_in(&name))
        .find(|path| is_file(path))
}

fn remove_other_versions(current: &str) {
    let Ok(entries) = fs::read_dir(".") else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(DIR_PREFIX) && name != current {
            fs::remove_dir_all(entry.path()).ok();
        }
    }
}

fn is_file(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// The language server doesn't run in the extension's working directory.
fn absolute(path: &str) -> Result<String> {
    let dir = env::current_dir().map_err(|err| format!("no working directory: {err}"))?;
    let path = dir.join(Path::new(path)).to_string_lossy().into_owned();

    // On Windows the working directory looks like `/C:/…`.
    Ok(match zed::current_platform().0 {
        Os::Windows => path.trim_start_matches('/').to_string(),
        Os::Mac | Os::Linux => path,
    })
}
