use std::path::Path;

pub fn get_lowercase_file_extension(file_path: &Path) -> Option<String> {
  file_path
    .extension()
    .and_then(|e| e.to_str())
    .map(|ext| String::from(ext).to_lowercase())
    .or_else(|| {
      if file_path.components().count() == 1 {
        let text = file_path.to_string_lossy();
        if let Some(index) = text.rfind('.')
          && index == 0
        {
          return Some(text[1..].to_lowercase());
        }
      }
      None
    })
}

pub fn get_lowercase_file_name(file_path: &Path) -> Option<String> {
  file_path.file_name().and_then(|s| s.to_str()).map(|s| s.to_lowercase())
}

/// Appends `.{ext}` to a path, preserving any existing name.
///
/// This differs from [`std::path::Path::with_extension`], which replaces the
/// existing extension. It is used to build the synthetic `path + mappedExt`
/// path for a shebang-routed extensionless file.
pub fn append_extension(file_path: &Path, ext: &str) -> std::path::PathBuf {
  let mut os_string = file_path.as_os_str().to_os_string();
  os_string.push(".");
  os_string.push(ext);
  std::path::PathBuf::from(os_string)
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn test_get_lowercase_file_extension() {
    assert_eq!(get_lowercase_file_extension(Path::new("test.txt")).unwrap(), "txt");
    assert_eq!(get_lowercase_file_extension(Path::new("test.txT")).unwrap(), "txt");
    assert_eq!(get_lowercase_file_extension(Path::new(".txt")).unwrap(), "txt");
    assert_eq!(get_lowercase_file_extension(Path::new(".Txt")).unwrap(), "txt");
    assert!(get_lowercase_file_extension(Path::new("txt")).is_none());
    assert!(get_lowercase_file_extension(Path::new("/path/.txt")).is_none());
    assert_eq!(get_lowercase_file_extension(Path::new("/path/test.txt")).unwrap(), "txt");
  }

  #[test]
  fn test_append_extension() {
    assert_eq!(append_extension(Path::new("temp-mean"), "ts"), Path::new("temp-mean.ts"));
    // only appends; the dot in the parent directory is untouched
    assert_eq!(append_extension(Path::new("/a.b/temp-mean"), "ts"), Path::new("/a.b/temp-mean.ts"));
  }
}
