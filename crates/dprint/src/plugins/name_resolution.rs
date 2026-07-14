use anyhow::Result;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use crate::environment::CanonicalizedPathBuf;
use crate::patterns::get_patterns_as_glob_matcher;
use crate::resolution::PluginWithConfig;
use crate::utils::GlobMatcher;
use crate::utils::GlobMatchesDetail;
use crate::utils::append_extension;
use crate::utils::get_lowercase_file_extension;
use crate::utils::get_lowercase_file_name;
use crate::utils::normalize_shebang_line;
use crate::utils::parse_shebang_line;
use crate::utils::shebang_line_matches_key;

#[derive(Default)]
pub struct PluginNameResolutionMaps {
  extension_to_plugin_names_map: HashMap<String, Vec<String>>,
  file_name_to_plugin_names_map: HashMap<String, Vec<String>>,
  /// Associations matchers ordered by precedence.
  association_matchers: Vec<(String, Rc<GlobMatcher>)>,
  /// Associations matchers in a map.
  association_matchers_map: HashMap<String, Rc<GlobMatcher>>,
  /// Normalized shebang line prefixes mapped to a file extension (without a
  /// leading dot), ordered longest-first so the most specific prefix wins.
  hashbang_extensions: Vec<(String, String)>,
}

impl PluginNameResolutionMaps {
  pub fn from_plugins<'a>(
    plugins: impl Iterator<Item = &'a PluginWithConfig>,
    config_base_path: &CanonicalizedPathBuf,
    hashbangs: &IndexMap<String, String>,
  ) -> Result<Self> {
    let mut plugin_name_maps = PluginNameResolutionMaps::default();
    for plugin in plugins {
      let plugin_name = plugin.name();

      for extension in &plugin.file_matching.file_extensions {
        plugin_name_maps
          .extension_to_plugin_names_map
          .entry(extension.to_lowercase())
          .or_default()
          .push(plugin_name.to_string());
      }
      for file_name in &plugin.file_matching.file_names {
        plugin_name_maps
          .file_name_to_plugin_names_map
          .entry(file_name.to_lowercase())
          .or_default()
          .push(plugin_name.to_string());
      }

      if let Some(matcher) = get_plugin_association_glob_matcher(plugin, config_base_path)? {
        let matcher = Rc::new(matcher);
        plugin_name_maps.association_matchers.push((plugin_name.to_string(), matcher.clone()));
        plugin_name_maps.association_matchers_map.insert(plugin_name.to_string(), matcher);
      }
    }
    // order longest-first so the most specific shebang prefix wins
    plugin_name_maps.hashbang_extensions = hashbangs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    plugin_name_maps.hashbang_extensions.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    Ok(plugin_name_maps)
  }

  /// Whether any `hashbangs` mapping is configured.
  pub fn has_hashbangs(&self) -> bool {
    !self.hashbang_extensions.is_empty()
  }

  /// Resolves the mapped file extension (without a leading dot) for a shebang
  /// line using longest-prefix matching at a whitespace boundary.
  pub fn resolve_hashbang_extension(&self, shebang_line: &str) -> Option<&str> {
    let normalized = normalize_shebang_line(shebang_line);
    self
      .hashbang_extensions
      .iter()
      .find(|(key, _)| shebang_line_matches_key(&normalized, key))
      .map(|(_, ext)| ext.as_str())
  }

  /// Resolves the plugin names and the (possibly synthetic) path to pass to the
  /// plugin for a file, taking an extensionless file's shebang into account.
  ///
  /// For a file routed via its shebang the returned path is `file_path` with the
  /// mapped extension appended (`path + mappedExt`); otherwise it is `file_path`.
  pub fn resolve_for_format(&self, file_path: &Path, file_bytes: &[u8]) -> (Vec<String>, PathBuf) {
    let plugin_names = self.get_plugin_names_from_file_path(file_path);
    if !plugin_names.is_empty() || !self.has_hashbangs() || get_lowercase_file_extension(file_path).is_some() {
      return (plugin_names, file_path.to_path_buf());
    }
    if let Some(shebang_line) = parse_shebang_line(file_bytes)
      && let Some(ext) = self.resolve_hashbang_extension(&shebang_line)
    {
      let synthetic_path = append_extension(file_path, ext);
      let names = self.get_plugin_names_from_file_path(&synthetic_path);
      if !names.is_empty() {
        return (names, synthetic_path);
      }
    }
    (plugin_names, file_path.to_path_buf())
  }

  pub fn get_plugin_names_from_file_path(&self, file_path: &Path) -> Vec<String> {
    let mut plugin_names = Vec::new();

    for (plugin_name, matcher) in self.association_matchers.iter() {
      if matcher.matches(file_path) {
        plugin_names.push(plugin_name.to_owned());
      }
    }

    if !plugin_names.is_empty() {
      return plugin_names;
    }

    if let Some(file_name) = get_lowercase_file_name(file_path)
      && let Some(plugin_names) = self.file_name_to_plugin_names_map.get(&file_name)
    {
      for plugin_name in plugin_names {
        if self.is_not_associations_excluded(plugin_name, file_path) {
          return vec![plugin_name.clone()];
        }
      }
    }

    if let Some(ext) = get_lowercase_file_extension(file_path)
      && let Some(plugin_names) = self.extension_to_plugin_names_map.get(&ext)
    {
      for plugin_name in plugin_names {
        if self.is_not_associations_excluded(plugin_name, file_path) {
          return vec![plugin_name.clone()];
        }
      }
    }

    plugin_names
  }

  fn is_not_associations_excluded(&self, plugin_name: &str, file_path: &Path) -> bool {
    // `associations` add to the plugin's default file matching, so a plugin
    // keeps matching by its default extension/file name unless a negated
    // association pattern explicitly excludes the file
    match self.association_matchers_map.get(plugin_name) {
      Some(matcher) => matcher.matches_detail(file_path) != GlobMatchesDetail::Excluded,
      None => true,
    }
  }
}

fn get_plugin_association_glob_matcher(plugin: &PluginWithConfig, config_base_path: &CanonicalizedPathBuf) -> Result<Option<GlobMatcher>> {
  match plugin.associations.as_deref() {
    Some(associations) => Ok(Some(get_patterns_as_glob_matcher(associations, config_base_path)?)),
    None => Ok(None),
  }
}
