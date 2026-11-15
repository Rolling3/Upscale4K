//! Windows: NVENC first, C:\OPFile\4K by default, ffmpeg from winget.

use std::env;
use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use upscale4k_application::ports::InstallHint;
use upscale4k_domain::encoder::Encoder;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use super::process_path_dirs;

const INSTALL_COMMAND: &str = "winget install --id Gyan.FFmpeg -e";
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

pub fn default_output_folder() -> PathBuf {
    PathBuf::from(r"C:\OPFile\4K")
}

pub fn preferred_encoders() -> Vec<Encoder> {
    vec![Encoder::Nvenc, Encoder::X264]
}

pub fn install_hint() -> InstallHint {
    let winget = winget_path().is_some();
    InstallHint {
        command: INSTALL_COMMAND.into(),
        can_run_in_app: winget,
        note: if winget {
            Some(
                "A terminal window opens and winget asks you to accept the package terms. Then choose Check again."
                    .into(),
            )
        } else {
            Some("winget isn't available. Install App Installer from the Microsoft Store, then run the command.".into())
        },
    }
}

pub fn write_blocked_hint(ffmpeg: Option<&Path>) -> Option<String> {
    let ffmpeg = ffmpeg.map(|p| format!(" and ffmpeg ({})", p.display())).unwrap_or_default();
    Some(format!(
        "Windows Controlled Folder Access may block this folder. To allow it, open Windows Security > \
         Virus & threat protection > Ransomware protection > Allow an app through Controlled folder access, \
         and add Upscale4K{ffmpeg}. Or choose another output folder."
    ))
}

pub fn ffmpeg_search_dirs() -> Vec<PathBuf> {
    let mut dirs = process_path_dirs();
    // A winget install changes PATH in the registry, not in this running process.
    dirs.extend(registry_path_dirs());
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join(r"Microsoft\WinGet\Links"));
        dirs.extend(winget_package_bins(&local.join(r"Microsoft\WinGet\Packages")));
    }
    if let Some(program_files) = env::var_os("ProgramFiles").map(PathBuf::from) {
        dirs.extend(winget_package_bins(&program_files.join(r"WinGet\Packages")));
    }
    if let Some(profile) = env::var_os("USERPROFILE").map(PathBuf::from) {
        dirs.push(profile.join(r"scoop\shims"));
    }
    // No guessed folders that any user can create, such as C:\ffmpeg or
    // C:\ProgramData\chocolatey (Chocolatey puts its folder on PATH anyway).
    dirs
}

/// `...\Packages\Gyan.FFmpeg_<source>\ffmpeg-<version>\bin`, newest name first.
fn winget_package_bins(packages: &Path) -> Vec<PathBuf> {
    let mut bins: Vec<PathBuf> = subdirs(packages)
        .into_iter()
        .filter(|p| name_starts_with(p, "Gyan.FFmpeg"))
        .flat_map(|pkg| subdirs(&pkg))
        .filter(|p| name_starts_with(p, "ffmpeg-"))
        .map(|p| p.join("bin"))
        .collect();
    bins.sort_by(|a, b| b.cmp(a));
    bins
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

fn name_starts_with(path: &Path, prefix: &str) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix))
}

/// Machine PATH, then user PATH, as stored in the registry right now.
fn registry_path_dirs() -> Vec<PathBuf> {
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
        .and_then(|key| key.get_value::<String, _>("Path"));
    let user =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment").and_then(|key| key.get_value::<String, _>("Path"));
    [machine, user]
        .into_iter()
        .flatten()
        .flat_map(|value| {
            value
                .split(';')
                .map(|part| part.trim().trim_matches('"').to_string())
                .filter(|part| !part.is_empty())
                .map(|part| PathBuf::from(expand_env(&part, &|name| env::var(name).ok())))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Expand `%NAME%` references; unknown names are left as they are.
fn expand_env(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match lookup(name).filter(|_| !name.is_empty()) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn winget_path() -> Option<PathBuf> {
    let mut dirs = process_path_dirs();
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join(r"Microsoft\WindowsApps"));
    }
    dirs.into_iter().map(|d| d.join("winget.exe")).find(|p| p.exists())
}

/// Opens a new console window running the winget command, so the user sees
/// the prompts and the result.
///
/// winget's folder is put first on PATH instead of into the command line,
/// because cmd.exe misreads quoted paths that contain `(`, `)` or `&`.
pub fn start_ffmpeg_install() -> io::Result<()> {
    let winget = winget_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "winget isn't available"))?;
    let mut path = winget.parent().map(|dir| dir.as_os_str().to_os_string()).unwrap_or_default();
    if let Some(current) = env::var_os("PATH") {
        path.push(";");
        path.push(current);
    }
    Command::new("cmd.exe")
        .args(["/k", "winget", "install", "--id", "Gyan.FFmpeg", "-e"])
        .env("PATH", path)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
}

/// Explorer often exits with code 1 even when it worked, so only the start is checked.
pub fn open_folder(dir: &Path) -> io::Result<()> {
    Command::new("explorer.exe").arg(dir).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "USERPROFILE" => Some(r"C:\Users\me".into()),
            "LOCALAPPDATA" => Some(r"C:\Users\me\AppData\Local".into()),
            _ => None,
        }
    }

    #[test]
    fn known_variables_are_expanded() {
        assert_eq!(expand_env(r"%USERPROFILE%\bin", &lookup), r"C:\Users\me\bin");
        assert_eq!(
            expand_env(r"%LOCALAPPDATA%\Microsoft\WindowsApps", &lookup),
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps"
        );
    }

    #[test]
    fn unknown_or_unclosed_variables_are_kept() {
        assert_eq!(expand_env(r"%NOPE%\bin", &lookup), r"%NOPE%\bin");
        assert_eq!(expand_env(r"C:\100%", &lookup), r"C:\100%");
        assert_eq!(expand_env("%%", &lookup), "%%");
    }

    #[test]
    fn winget_package_folders_are_found_newest_first() {
        let root = tempfile::tempdir().unwrap();
        let pkg = root.path().join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe");
        for version in ["ffmpeg-8.1-full_build", "ffmpeg-9.0.2-full_build"] {
            fs::create_dir_all(pkg.join(version).join("bin")).unwrap();
        }
        fs::create_dir_all(root.path().join("Other.Package").join("ffmpeg-1")).unwrap();
        assert_eq!(
            winget_package_bins(root.path()),
            vec![pkg.join("ffmpeg-9.0.2-full_build").join("bin"), pkg.join("ffmpeg-8.1-full_build").join("bin")]
        );
    }

    #[test]
    fn the_blocked_folder_hint_names_controlled_folder_access_and_ffmpeg() {
        let hint = write_blocked_hint(Some(Path::new(r"C:\tools\ffmpeg.exe"))).unwrap();
        assert!(hint.contains("Controlled Folder Access"), "{hint}");
        assert!(hint.contains(r"C:\tools\ffmpeg.exe"), "{hint}");
    }
}
