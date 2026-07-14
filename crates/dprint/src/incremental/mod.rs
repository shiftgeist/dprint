mod incremental_file;

pub use incremental_file::IncrementalFile;

use std::hash::Hasher;

use crate::configuration::ResolvedConfig;
use crate::environment::Environment;
use crate::resolution::PluginsScope;
use crate::utils::FastInsecureHasher;
use crate::utils::get_bytes_hash;

pub fn get_incremental_file<TEnvironment: Environment>(
  incremental_cli_arg: Option<bool>,
  config: &ResolvedConfig,
  scope: &PluginsScope<TEnvironment>,
  environment: &TEnvironment,
) -> Option<IncrementalFile<TEnvironment>> {
  if let Some(incremental_arg) = incremental_cli_arg.or(config.incremental)
    && !incremental_arg
  {
    return None;
  }

  // the incremental file is stored in the cache with a key based on the root directory
  let incremental_dir = environment.get_cache_dir().join_panic_relative("incremental");
  if environment.mk_dir_all(&incremental_dir).is_err() {
    return None;
  }

  // fold the hashbangs into the state hash so changing the mapping invalidates
  // the cache (the plugins hash on its own wouldn't reflect a hashbangs change)
  let mut hasher = FastInsecureHasher::default();
  hasher.write_u64(scope.plugins_hash());
  if config.hashbangs_enabled() {
    for (key, ext) in &config.hashbangs {
      hasher.write(key.as_bytes());
      hasher.write(ext.as_bytes());
    }
  }
  let state_hash = hasher.finish();

  let base_path = config.base_path.clone();
  let file_path = incremental_dir.join_panic_relative(get_bytes_hash(base_path.to_string_lossy().as_bytes()).to_string());
  Some(IncrementalFile::new(file_path, state_hash, environment.clone()))
}
