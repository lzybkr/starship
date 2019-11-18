use unicode_segmentation::UnicodeSegmentation;
use std::path::{Component, Path, PathBuf, Prefix};

use super::{Context, Module};

use super::utils::directory::truncate;
use crate::config::{RootModuleConfig, SegmentConfig};
use crate::configs::directory::DirectoryConfig;

/// Creates a module with the current directory
///
/// Will perform path contraction and truncation.
/// **Contraction**
///     - Paths beginning with the home directory or with a git repo right
/// inside the home directory will be contracted to `~`
///     - Paths containing a git repo will contract to begin at the repo root
///
/// **Truncation**
/// Paths will be limited in length to `3` path components by default.
pub fn module<'a>(context: &'a Context) -> Option<Module<'a>> {
    const HOME_SYMBOL: &str = "~";
    const ELLIPSIS: &str = "\u{2026}";

    let mut module = context.new_module("directory");
    let config: DirectoryConfig = DirectoryConfig::try_load(module.config);

    module.set_style(config.style);

    // Using environment PWD is the standard approach for determining logical path
    // If this is None for any reason, we fall back to reading the os-provided path
    let physical_current_dir = if config.use_logical_path {
        None
    } else {
        match std::env::current_dir() {
            Ok(x) => Some(x),
            Err(e) => {
                log::debug!("Error getting physical current directory: {}", e);
                None
            }
        }
    };
    let current_dir = Path::new(
        physical_current_dir
            .as_ref()
            .unwrap_or_else(|| &context.current_dir),
    );

    let home_dir = dirs::home_dir().unwrap();
    log::debug!("Current directory: {:?}", current_dir);

    let repo = &context.get_repo().ok()?;

    let home_dir_contracted = contract_path(&current_dir, &home_dir, HOME_SYMBOL);
    let components = home_dir_contracted.components().collect::<Vec<_>>();
    let (prefix, path_parts): (Vec<Component>, Vec<Component>) =
        components.into_iter().partition(|c| match c {
            Component::Prefix(_) | Component::RootDir => true,
            _ => false,
        });

    let mut result = String::new();
    let separator = get_separator();
    for component in prefix {
        match component {
            Component::Prefix(prefix) => {
                result.push_str(&get_windows_prefix(prefix.kind(), separator));
            }
            Component::RootDir => result.push_str(separator),
            _ => unreachable!(),
        }
    }

    let first_full_part = path_parts
        .len()
        .saturating_sub(config.truncation_length as usize);

    let truncated_parts = &path_parts[0..first_full_part];
    if truncated_parts.len() > 0 {
        if config.fish_style_pwd_dir_length > 0 {
            let truncated_part = truncated_parts
                .iter()
                .map(|c| match c {
                    Component::CurDir => ".".to_string(),
                    Component::ParentDir => "..".to_string(),
                    Component::Normal(p) => p.to_string_lossy()
                        [0..config.fish_style_pwd_dir_length as usize]
                        .to_string(),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>()
                .join(separator);
            result.push_str(&truncated_part);
        } else {
            // Replace truncated portion with ellipsis.
            result.push_str(ELLIPSIS);
        }
        result.push_str(separator);
    }

    let full_part = path_parts[first_full_part..]
        .iter()
        .map(|c| match c {
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
            Component::Normal(p) => p.to_string_lossy().into_owned(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>()
        .join(separator);
    result.push_str(&full_part);

    module.create_segment(
        "path",
        &SegmentConfig {
            value: &result,
            style: None,
        },
    );

    /*
    if config.truncate_to_repo {
        if let Some(repo_root) = &repo.root {
            contract_path(&repo_root, &home_dir, HOME_SYMBOL);
        }
    }
    let contracted_path = match &repo.root {
        Some(repo_root) if config.truncate_to_repo && (repo_root != &home_dir) => {
            let repo_folder_name = repo_root.file_name().unwrap().to_str().unwrap();

            // Contract the path to the git repo root
            contract_path(current_dir, repo_root, repo_folder_name)
        }
        // Contract the path to the home directory
        _ => contract_path(current_dir, &home_dir, HOME_SYMBOL),
    };

    // Truncate the dir string to the maximum number of path components
    let truncated_dir_string = truncate(&contracted_path, config.truncation_length as usize);

    if config.fish_style_pwd_dir_length > 0 {
        // If user is using fish style path, we need to add the segment first
        let fish_style_dir = to_fish_style(
            config.fish_style_pwd_dir_length as usize,
            contracted_home_dir,
            &truncated_dir_string,
        );
    }

    */

    module.get_prefix().set_value(config.prefix);

    Some(module)
}

fn get_separator() -> &'static str {
   match std::env::var("STARSHIP_SHELL").unwrap_or_default().as_str() {
        "bash" | "zsh" | "fish" => "/",
        _ => {
            if cfg!(windows) {
                "\\"
            } else {
                "/"
            }
        }
    }
}

fn get_windows_prefix(prefix: Prefix, separator: &str) -> String {
    let mut buf = String::with_capacity(3);

    match prefix {
        Prefix::Disk(disk) | Prefix::VerbatimDisk(disk) => {
            // c: or \\?\c:
            if separator.chars().next() == Some('/') {
                buf.push('/');
                buf.push((disk as char).to_ascii_lowercase());
            } else {
                buf.push((disk as char).to_ascii_lowercase());
                buf.push(':');
            }
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            // \\server\share or // \\?\UNC\server\share
            buf.push_str(separator);
            buf.push_str(separator);
            buf.push_str(server.to_string_lossy().as_ref());
            buf.push_str(separator);
            buf.push_str(share.to_string_lossy().as_ref());
        }
        Prefix::Verbatim(path) | Prefix::DeviceNS(path) => {
            // \\?\path or \\.\COM1
            buf.push_str(&path.to_string_lossy())
        }
    };

    buf
}

/// Contract the root component of a path
///
/// Replaces the `top_level_path` in a given `full_path` with the provided
/// `top_level_replacement`.
fn contract_path(full_path: &Path, top_level_path: &Path, top_level_replacement: &str) -> PathBuf {
    match full_path.strip_prefix(top_level_path) {
        Ok(p) => {
            let mut contracted_path = PathBuf::from(top_level_replacement);
            if p.to_str() != Some("") {
                contracted_path.push(p);
            }
            contracted_path
        }
        Err(_) => full_path.into(),
    }
}

/// Truncate a path to only have a set number of path components
///
/// Will truncate a path to only show the last `length` components in a path.
/// If a length of `0` is provided, the path will not be truncated.
/*
fn truncate(contracted_path: &Path, length: usize) -> String {
    if length == 0 {
        return dir_string;
    }

    let mut components = dir_string.split('/').collect::<Vec<&str>>();

    // If the first element is "" then there was a leading "/" and we should remove it so we can check the actual count of components
    if components[0] == "" {
        components.remove(0);
    }

    if components.len() <= length {
        return dir_string;
    }

    let truncated_components = &components[components.len() - length..];
    truncated_components.join("/")
}

/// Takes part before contracted path and replaces it with fish style path
///
/// Will take the first letter of each directory before the contracted path and
/// use that in the path instead. See the following example.
///
/// Absolute Path: `/Users/Bob/Projects/work/a_repo`
/// Contracted Path: `a_repo`
/// With Fish Style: `~/P/w/a_repo`
///
/// Absolute Path: `/some/Path/not/in_a/repo/but_nested`
/// Contracted Path: `in_a/repo/but_nested`
/// With Fish Style: `/s/P/n/in_a/repo/but_nested`
fn to_fish_style(pwd_dir_length: usize, dir_string: String, truncated_dir_string: &str) -> String {
    let replaced_dir_string = dir_string.trim_end_matches(truncated_dir_string).to_owned();
    let components = replaced_dir_string.split('/').collect::<Vec<&str>>();

    if components.is_empty() {
        return replaced_dir_string;
    }

    components
        .into_iter()
        .map(|word| -> String {
            let chars = UnicodeSegmentation::graphemes(word, true).collect::<Vec<&str>>();
            match word {
                "" => "".to_string(),
                _ if chars.len() <= pwd_dir_length => word.to_string(),
                _ if word.starts_with('.') => chars[..=pwd_dir_length].join(""),
                _ => chars[..pwd_dir_length].join(""),
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
*/

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_home_directory() {
        let full_path = Path::new("/Users/astronaut/schematics/rocket");
        let home = Path::new("/Users/astronaut");

        let output = contract_path(full_path, home, "~");
        assert_eq!(output, "~/schematics/rocket");
    }

    #[test]
    fn contract_repo_directory() {
        let full_path = Path::new("/Users/astronaut/dev/rocket-controls/src");
        let repo_root = Path::new("/Users/astronaut/dev/rocket-controls");

        let output = contract_path(full_path, repo_root, "rocket-controls");
        assert_eq!(output, "rocket-controls/src");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn contract_windows_style_home_directory() {
        let full_path = Path::new("C:\\Users\\astronaut\\schematics\\rocket");
        let home = Path::new("C:\\Users\\astronaut");

        let output = contract_path(full_path, home, "~");
        assert_eq!(output, "~\\schematics\\rocket");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn contract_windows_style_repo_directory() {
        let full_path = Path::new("C:\\Users\\astronaut\\dev\\rocket-controls\\src");
        let repo_root = Path::new("C:\\Users\\astronaut\\dev\\rocket-controls");

        let output = contract_path(full_path, repo_root, "rocket-controls");
        assert_eq!(output, "rocket-controls\\src");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn contract_windows_style_no_top_level_directory() {
        let full_path = Path::new("C:\\Some\\Other\\Path");
        let top_level_path = Path::new("C:\\Users\\astronaut");

        let output = contract_path(full_path, top_level_path, "~");
        assert_eq!(output, "/c/Some/Other/Path");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn contract_windows_style_root_directory() {
        let full_path = Path::new("C:\\");
        let top_level_path = Path::new("C:\\Users\\astronaut");

        let output = contract_path(full_path, top_level_path, "~");
        assert_eq!(output, "/c");
    }

    #[test]
    fn fish_style_with_user_home_contracted_path() {
        let path = "~/starship/engines/booster/rocket";
        let output = to_fish_style(1, path.to_string(), "engines/booster/rocket");
        assert_eq!(output, "~/s/");
    }

    #[test]
    fn fish_style_with_user_home_contracted_path_and_dot_dir() {
        let path = "~/.starship/engines/booster/rocket";
        let output = to_fish_style(1, path.to_string(), "engines/booster/rocket");
        assert_eq!(output, "~/.s/");
    }

    #[test]
    fn fish_style_with_no_contracted_path() {
        // `truncatation_length = 2`
        let path = "/absolute/Path/not/in_a/repo/but_nested";
        let output = to_fish_style(1, path.to_string(), "repo/but_nested");
        assert_eq!(output, "/a/P/n/i/");
    }

    #[test]
    fn fish_style_with_pwd_dir_len_no_contracted_path() {
        // `truncatation_length = 2`
        let path = "/absolute/Path/not/in_a/repo/but_nested";
        let output = to_fish_style(2, path.to_string(), "repo/but_nested");
        assert_eq!(output, "/ab/Pa/no/in/");
    }

    #[test]
    fn fish_style_with_duplicate_directories() {
        let path = "~/starship/tmp/C++/C++/C++";
        let output = to_fish_style(1, path.to_string(), "C++");
        assert_eq!(output, "~/s/t/C/C/");
    }

    #[test]
    fn fish_style_with_unicode() {
        let path = "~/starship/tmp/目录/a̐éö̲/目录";
        let output = to_fish_style(1, path.to_string(), "目录");
        assert_eq!(output, "~/s/t/目/a̐/");
    }
}
