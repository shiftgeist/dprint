use anyhow::Context;
use anyhow::Result;
use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::str::Split;
use sys_traits::FsMetadata;
use thiserror::Error;

use crate::arg_parser::ConfigDiscovery;
use crate::arg_parser::FilePatternArgs;
use crate::configuration::ResolvedConfig;
use crate::environment::CanonicalizedPathBuf;
use crate::environment::Environment;
use crate::patterns::get_all_file_patterns;
use crate::patterns::process_config_patterns;
use crate::plugins::PluginNameResolutionMaps;
use crate::resolution::PluginWithConfig;
use crate::utils::GlobOptions;
use crate::utils::GlobOutput;
use crate::utils::GlobPattern;
use crate::utils::GlobPatterns;
use crate::utils::SHEBANG_READ_BYTES;
use crate::utils::append_extension;
use crate::utils::get_lowercase_file_extension;
use crate::utils::glob;
use crate::utils::is_negated_glob;
use crate::utils::is_pattern;
use crate::utils::parse_shebang_line;

/// Struct that allows using plugin names as a key
/// in a hash map.
#[derive(Debug, Eq, PartialEq, Hash)]
pub struct PluginNames(String);

impl PluginNames {
  const SEPARATOR: &'static str = "~~";

  pub fn from_plugin_names(names: &[String]) -> Self {
    Self(names.join(PluginNames::SEPARATOR))
  }

  pub fn names(&self) -> Split<'_, &str> {
    self.0.split(PluginNames::SEPARATOR)
  }
}

#[derive(Debug, Error)]
#[error("No files found to format with the specified plugins at {}. You may want to try using `dprint output-file-paths` to see which files it's finding or run with `--allow-no-files`.", .base_path.display())]
pub struct NoFilesFoundError {
  pub base_path: CanonicalizedPathBuf,
}

/// A file to format along with the path that should be passed to the plugin.
///
/// For a normal file `format_ext` is `None` and the plugin sees `path`. For an
/// extensionless file routed via its shebang, `format_ext` holds the mapped
/// extension so the plugin sees `path + mappedExt`, while the file is still read
/// from and written to `path`.
pub struct FileToFormat {
  pub path: PathBuf,
  pub format_ext: Option<String>,
}

impl FileToFormat {
  /// The path to pass to the plugin for formatting.
  pub fn format_path(&self) -> Cow<'_, Path> {
    match &self.format_ext {
      Some(ext) => Cow::Owned(append_extension(&self.path, ext)),
      None => Cow::Borrowed(&self.path),
    }
  }
}

pub struct FilesPathsByPlugins(HashMap<PluginNames, Vec<FileToFormat>>);

impl FilesPathsByPlugins {
  pub fn ensure_not_empty(&self, base_path: &CanonicalizedPathBuf) -> Result<(), NoFilesFoundError> {
    if self.is_empty() {
      Err(NoFilesFoundError { base_path: base_path.clone() })
    } else {
      Ok(())
    }
  }

  pub fn is_empty(&self) -> bool {
    self.0.is_empty()
  }

  pub fn into_vec(self) -> Vec<(PluginNames, Vec<FileToFormat>)> {
    self.0.into_iter().collect()
  }

  pub fn all_file_paths(&self) -> impl Iterator<Item = &PathBuf> {
    self.0.values().flatten().map(|f| &f.path)
  }
}

pub fn get_file_paths_by_plugins(
  plugin_name_maps: &PluginNameResolutionMaps,
  file_paths: Vec<PathBuf>,
  environment: &impl Environment,
) -> Result<FilesPathsByPlugins> {
  let mut file_paths_by_plugin: HashMap<PluginNames, Vec<FileToFormat>> = HashMap::new();

  for file_path in file_paths.into_iter() {
    let mut plugin_names = plugin_name_maps.get_plugin_names_from_file_path(&file_path);
    let mut format_ext = None;

    // fall back to shebang-based routing for extensionless files that didn't
    // match by association, file name or extension
    if plugin_names.is_empty()
      && plugin_name_maps.has_hashbangs()
      && get_lowercase_file_extension(&file_path).is_none()
      && let Some(shebang_line) = read_shebang_line(environment, &file_path)
      && let Some(ext) = plugin_name_maps.resolve_hashbang_extension(&shebang_line)
    {
      let synthetic_path = append_extension(&file_path, ext);
      let names = plugin_name_maps.get_plugin_names_from_file_path(&synthetic_path);
      if !names.is_empty() {
        plugin_names = names;
        format_ext = Some(ext.to_string());
      }
    }

    if !plugin_names.is_empty() {
      let plugin_names_key = PluginNames::from_plugin_names(&plugin_names);
      let files = file_paths_by_plugin.entry(plugin_names_key).or_default();
      files.push(FileToFormat { path: file_path, format_ext });
    }
  }

  Ok(FilesPathsByPlugins(file_paths_by_plugin))
}

/// Reads the shebang line from the start of an extensionless file, if present.
fn read_shebang_line(environment: &impl Environment, file_path: &Path) -> Option<String> {
  let bytes = environment.read_file_start_bytes(file_path, SHEBANG_READ_BYTES).ok()?;
  parse_shebang_line(&bytes)
}

pub async fn get_and_resolve_file_paths<'a>(
  config: &ResolvedConfig,
  args: &FilePatternArgs,
  config_discovery: ConfigDiscovery,
  plugins: impl Iterator<Item = &'a PluginWithConfig>,
  environment: &impl Environment,
) -> Result<GlobOutput> {
  let cwd = environment.cwd();
  let args = expand_directory_include_patterns(args, environment);
  let mut file_patterns = get_all_file_patterns(config, &args, &cwd);

  if args.only_staged {
    let staged_files = environment.get_staged_files().context("Failed running git staged.")?;
    file_patterns.arg_includes = Some(GlobPattern::new_vec(
      staged_files.into_iter().map(|path| path.to_string_lossy().into_owned()).collect(),
      cwd.clone(),
    ));
  } else if args.only_dirty {
    let dirty_files = environment.get_dirty_files().context("Failed running git status.")?;
    file_patterns.arg_includes = Some(GlobPattern::new_vec(
      dirty_files.into_iter().map(|path| path.to_string_lossy().into_owned()).collect(),
      cwd.clone(),
    ));
  }

  if file_patterns.config_includes.is_none() {
    // If no includes patterns were specified, derive one from the list of plugins
    // as this is a massive performance improvement, because it collects less file
    // paths to examine and match to plugins later.
    let search_base = get_cli_search_base(&cwd, &file_patterns);
    let mut patterns = get_plugin_patterns(plugins);
    if config.hashbangs_enabled() {
      // when shebang routing is enabled, also collect extensionless files so
      // they can be routed to a plugin based on their shebang line
      patterns.push("**/*".to_string());
    }
    file_patterns.config_includes = Some(GlobPattern::new_vec(patterns, search_base));
  }

  get_and_resolve_file_patterns(config, file_patterns, args.no_gitignore, config_discovery, environment).await
}

fn expand_directory_include_patterns(args: &FilePatternArgs, environment: &impl Environment) -> FilePatternArgs {
  FilePatternArgs {
    include_patterns: args
      .include_patterns
      .iter()
      .flat_map(|pattern| expand_directory_include_pattern(pattern, environment))
      .collect(),
    ..args.clone()
  }
}

fn expand_directory_include_pattern(pattern: &str, environment: &impl Environment) -> Vec<String> {
  if pattern == "." || is_pattern(pattern) {
    return vec![pattern.to_string()];
  }

  let normalized_pattern = pattern.replace('\\', "/");
  let pattern_path = PathBuf::from(&normalized_pattern);
  let path = if environment.is_absolute_path(&pattern_path) {
    pattern_path
  } else {
    environment.cwd().join(pattern_path)
  };
  if environment.fs_is_dir_no_err(path) {
    vec![normalized_pattern.clone(), format!("{}/**/*", normalized_pattern.trim_end_matches('/'))]
  } else {
    vec![pattern.to_string()]
  }
}

async fn get_and_resolve_file_patterns(
  config: &ResolvedConfig,
  file_patterns: GlobPatterns,
  no_gitignore: bool,
  config_discovery: ConfigDiscovery,
  environment: &impl Environment,
) -> Result<GlobOutput> {
  let cwd = environment.cwd();
  let is_cwd_in_base = cwd.starts_with(&config.base_path);
  let is_in_sub_dir = cwd != config.base_path && is_cwd_in_base;
  let start_dir = if is_in_sub_dir {
    get_cli_search_base(&cwd, &file_patterns)
  } else {
    config.base_path.clone()
  };
  let environment = environment.clone();
  let pattern_base = config.base_path.clone();

  // This is intensive so do it in a blocking task
  dprint_core::async_runtime::spawn_blocking(move || {
    glob(
      &environment,
      GlobOptions {
        start_dir: start_dir.into_path_buf(),
        file_patterns,
        pattern_base,
        config_discovery,
        no_gitignore,
      },
    )
  })
  .await
  .unwrap()
}

fn get_cli_search_base(cwd: &CanonicalizedPathBuf, file_patterns: &GlobPatterns) -> CanonicalizedPathBuf {
  file_patterns
    .arg_includes
    .iter()
    .flat_map(|patterns| patterns.iter())
    .filter(|pattern| !pattern.is_negated() && cwd.starts_with(&pattern.base_dir))
    .fold(cwd.clone(), |base_dir, pattern| {
      if base_dir.starts_with(&pattern.base_dir) {
        pattern.base_dir.clone()
      } else {
        base_dir
      }
    })
}

fn get_plugin_patterns<'a>(plugins: impl Iterator<Item = &'a PluginWithConfig>) -> Vec<String> {
  let mut file_names = HashSet::new();
  let mut file_exts = HashSet::new();
  let mut association_globs = Vec::new();
  for plugin in plugins {
    // associations add to the plugin's default file matching, so always include
    // the plugin's default file names and extensions plus any positive globs
    file_names.extend(&plugin.file_matching.file_names);
    file_exts.extend(&plugin.file_matching.file_extensions);
    if let Some(associations) = plugin.associations.as_ref() {
      for pattern in process_config_patterns(associations) {
        if !is_negated_glob(&pattern) {
          association_globs.push(pattern);
        }
      }
    }
  }
  let mut result = Vec::new();
  if !file_exts.is_empty() {
    result.push(format!("**/*.{{{}}}", file_exts.into_iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")));
  }
  if !file_names.is_empty() {
    result.push(format!("**/{{{}}}", file_names.into_iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")));
  }
  // add the association globs last as they're least likely to be matched
  result.extend(association_globs);

  result
}
