//! Git 변경 캡처의 데이터 전용 Cargo 설정. 일반 load의 호출 디렉터리 정책과 분리한다.
use super::{create_private_dir, metadata_failure, parse, process_toolchain, Metadata, Tool};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 2 * MAX_CONFIG_BYTES;
const MAX_FILES: usize = 32;
const MAX_ROWS: usize = 256;
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// 검증한 설정과 그 설정이 속한 캡처를 함께 넘겨 임의 caller CWD 설정을 빌리지 않는다.
pub(crate) struct CaptureContext {
    project: PathBuf,
    config: SafeConfig,
    files: usize,
}

impl CaptureContext {
    pub(crate) fn new(root: &Path, project: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|_| invalid("cannot resolve capture root"))?;
        if !checked_metadata(&root, project)?.is_some_and(|metadata| metadata.is_dir()) {
            return Err(invalid(
                "captured project must be a regular directory inside its canonical revision",
            ));
        }
        let project = project
            .canonicalize()
            .map_err(|_| invalid("cannot resolve captured project"))?;
        if !project.starts_with(&root) || !project.is_dir() {
            return Err(invalid("captured project is outside its revision"));
        }
        let mut directories: Vec<_> = project
            .ancestors()
            .take_while(|dir| dir.starts_with(&root))
            .collect();
        directories.reverse();
        let mut config = SafeConfig::default();
        let mut files = 0;
        let mut bytes = 0;
        for directory in directories {
            let cargo = directory.join(".cargo");
            let Some(metadata) = checked_metadata(&root, &cargo)? else {
                continue;
            };
            if !metadata.is_dir() {
                return Err(invalid(".cargo must be a regular directory"));
            }
            let names = [cargo.join("config"), cargo.join("config.toml")];
            let mut selected = None;
            for path in names {
                if let Some(metadata) = checked_metadata(&root, &path)? {
                    if !metadata.is_file() {
                        return Err(invalid("configuration must be a regular file"));
                    }
                    if selected.is_some() {
                        return Err(invalid("keep only one of .cargo/config and config.toml"));
                    }
                    selected = Some(path);
                }
            }
            if let Some(path) = selected {
                files += 1;
                if files > MAX_FILES {
                    return Err(invalid("configuration file limit exceeded"));
                }
                let mut data = Vec::new();
                std::fs::File::open(path)
                    .and_then(|file| file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut data))
                    .map_err(|_| invalid("cannot read configuration"))?;
                bytes += data.len() as u64;
                if data.len() as u64 > MAX_CONFIG_BYTES || bytes > MAX_TOTAL_BYTES {
                    return Err(invalid("configuration byte limit exceeded"));
                }
                let text = std::str::from_utf8(&data)
                    .map_err(|_| invalid("configuration must be UTF-8 TOML"))?;
                let table: toml::Table = text
                    .parse()
                    .map_err(|_| invalid("configuration is not valid TOML"))?;
                config.merge(table, &root, directory)?;
            }
        }
        config.validate_sources()?;
        Ok(Self {
            project,
            config,
            files,
        })
    }

    pub(crate) fn project(&self) -> &Path {
        &self.project
    }

    pub(crate) fn load(&self) -> Result<Metadata, String> {
        #[cfg(test)]
        super::LOADS.with(|counter| counter.set(counter.get() + 1));
        let caller =
            std::env::current_dir().map_err(|_| invalid("cannot anchor the caller environment"))?;
        for name in ["RUSTUP_HOME", "HOME", "USERPROFILE"] {
            if std::env::var_os(name)
                .is_some_and(|value| value.is_empty() || !Path::new(&value).is_absolute())
            {
                return Err(invalid("relative Rustup/home selection needs an absolute path before isolated toolchain resolution"));
            }
        }
        if std::env::var_os("RUSTUP_TOOLCHAIN").is_some_and(|value| {
            !Path::new(&value).is_absolute() && Path::new(&value).components().count() > 1
        }) {
            return Err(invalid(
                "relative toolchain directories need an absolute path before isolated resolution",
            ));
        }
        let scratch = Scratch::new()?;
        refuse_ancestor_configuration(&scratch.0)?;
        let config = scratch.0.join("capture-config.toml");
        let text = self.config.render()?;
        let mut file = std::fs::OpenOptions::new();
        file.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file.mode(0o600);
        }
        file.open(&config)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(|_| invalid("cannot write private derived configuration"))?;
        let toolchain = process_toolchain()
            .map_err(|error| format!("failed to run captured Cargo metadata: {error}"))?;
        let mut command = toolchain.command_for(Tool::Cargo)?;
        anchor_environment(&mut command, &caller)?;
        if let Some(rustc) = &toolchain.rustc {
            let path = Path::new(rustc);
            if !path.is_absolute() && path.components().count() > 1 {
                command.env("RUSTC", caller.join(path));
            }
        }
        let output = command
            .current_dir(&scratch.0)
            .args(["metadata", "--offline", "--format-version", "1", "--config"])
            .arg(&config)
            .arg("--manifest-path")
            .arg(self.project.join("Cargo.toml"))
            .output()
            .map_err(|_| invalid("cannot start offline Cargo metadata"))?;
        if !output.status.success() {
            return Err(metadata_failure(&self.project, &output.stderr, toolchain));
        }
        let mut metadata = parse(&output.stdout)?;
        if toolchain.ignored_toolchain_file {
            metadata
                .limitations
                .push(super::TOOLCHAIN_FILE_IGNORED.into());
        }
        metadata.limitations.push(format!("Captured Cargo metadata used {} validated revision-local data configuration files and an offline neutral working directory; the caller-selected Cargo home and environment remain external trusted inputs.", self.files));
        Ok(metadata)
    }
}

fn invalid(reason: &str) -> String {
    format!("captured Cargo configuration: {reason}; use supported revision-local paths, path-only patches or directory-source replacements, or analyze that project separately")
}

#[derive(Default)]
struct SafeConfig {
    paths: Vec<PathBuf>,
    patches: BTreeMap<String, BTreeMap<String, PathBuf>>,
    sources: BTreeMap<String, Source>,
}

enum Source {
    Directory(PathBuf),
    Replace(String),
}

impl SafeConfig {
    fn merge(&mut self, table: toml::Table, root: &Path, directory: &Path) -> Result<(), String> {
        for (key, value) in table {
            match key.as_str() {
                "paths" => {
                    for value in value
                        .as_array()
                        .ok_or_else(|| invalid("paths must be an array"))?
                    {
                        self.paths
                            .push(local_directory(root, directory, text(value)?)?);
                        if self.paths.len() > MAX_ROWS {
                            return Err(invalid("path row limit exceeded"));
                        }
                    }
                }
                "patch" => {
                    for (registry, packages) in value
                        .as_table()
                        .ok_or_else(|| invalid("patch must be a table"))?
                    {
                        portable_name(registry)?;
                        for (package, fields) in packages
                            .as_table()
                            .ok_or_else(|| invalid("patch registry must be a table"))?
                        {
                            portable_name(package)?;
                            let fields = fields.as_table().ok_or_else(|| {
                                invalid("patch package must be a path-only table")
                            })?;
                            if fields.len() != 1 || !fields.contains_key("path") {
                                return Err(invalid("patches must contain only a local path"));
                            }
                            let path = local_directory(root, directory, text(&fields["path"])?)?;
                            self.patches
                                .entry(registry.clone())
                                .or_default()
                                .insert(package.clone(), path);
                        }
                    }
                    if self.patches.values().map(BTreeMap::len).sum::<usize>() > MAX_ROWS {
                        return Err(invalid("patch row limit exceeded"));
                    }
                }
                "source" => {
                    for (name, fields) in value
                        .as_table()
                        .ok_or_else(|| invalid("source must be a table"))?
                    {
                        portable_name(name)?;
                        let fields = fields.as_table().ok_or_else(|| {
                            invalid("source must be a directory or replacement table")
                        })?;
                        if fields.len() != 1 {
                            return Err(invalid(
                                "source must contain one directory or replace-with field",
                            ));
                        }
                        let source = if let Some(value) = fields.get("directory") {
                            Source::Directory(local_directory(root, directory, text(value)?)?)
                        } else if let Some(value) = fields.get("replace-with") {
                            let name = text(value)?;
                            portable_name(name)?;
                            Source::Replace(name.into())
                        } else {
                            return Err(invalid(
                                "only local directory source replacements are supported",
                            ));
                        };
                        if self.sources.get(name).is_some_and(|old| {
                            std::mem::discriminant(old) != std::mem::discriminant(&source)
                        }) {
                            return Err(invalid(
                                "source kind changes across config levels are ambiguous",
                            ));
                        }
                        self.sources.insert(name.clone(), source);
                    }
                    if self.sources.len() > MAX_ROWS {
                        return Err(invalid("source row limit exceeded"));
                    }
                }
                _ => {
                    return Err(invalid(
                        "unsupported section or environment/program setting",
                    ))
                }
            }
        }
        Ok(())
    }

    fn validate_sources(&self) -> Result<(), String> {
        for name in self.sources.keys() {
            let mut current = name;
            let mut visited = BTreeSet::new();
            loop {
                if !visited.insert(current) {
                    return Err(invalid("source replacement cycle"));
                }
                match self.sources.get(current) {
                    Some(Source::Directory(_)) => break,
                    Some(Source::Replace(next)) => current = next,
                    None => {
                        return Err(invalid(
                            "source replacement must end in a revision-local directory",
                        ))
                    }
                }
            }
        }
        Ok(())
    }

    fn render(&self) -> Result<String, String> {
        let path_value = |path: &Path| {
            path.to_str()
                .map(|value| toml::Value::String(value.into()))
                .ok_or_else(|| invalid("local paths must be representable as UTF-8"))
        };
        let mut table = toml::Table::new();
        if !self.paths.is_empty() {
            table.insert(
                "paths".into(),
                toml::Value::Array(
                    self.paths
                        .iter()
                        .map(|path| path_value(path))
                        .collect::<Result<_, _>>()?,
                ),
            );
        }
        let mut patches = toml::Table::new();
        for (registry, packages) in &self.patches {
            let mut values = toml::Table::new();
            for (package, path) in packages {
                values.insert(
                    package.clone(),
                    toml::Value::Table(toml::Table::from_iter([(
                        "path".into(),
                        path_value(path)?,
                    )])),
                );
            }
            patches.insert(registry.clone(), toml::Value::Table(values));
        }
        if !patches.is_empty() {
            table.insert("patch".into(), toml::Value::Table(patches));
        }
        let mut sources = toml::Table::new();
        for (name, source) in &self.sources {
            let (key, value) = match source {
                Source::Directory(path) => ("directory", path_value(path)?),
                Source::Replace(name) => ("replace-with", toml::Value::String(name.clone())),
            };
            sources.insert(
                name.clone(),
                toml::Value::Table(toml::Table::from_iter([(key.into(), value)])),
            );
        }
        if !sources.is_empty() {
            table.insert("source".into(), toml::Value::Table(sources));
        }
        let text = table.to_string();
        if text.len() as u64 > MAX_TOTAL_BYTES {
            return Err(invalid("derived configuration byte limit exceeded"));
        }
        Ok(text)
    }
}

fn text(value: &toml::Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| invalid("expected a string value"))
}

fn portable_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid("unsupported source, registry or package name"));
    }
    Ok(())
}

fn local_directory(root: &Path, directory: &Path, value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || Path::new(value).is_absolute()
    {
        return Err(invalid("local directories must use bounded relative paths"));
    }
    let mut path = directory.to_path_buf();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            Component::ParentDir if path != root => {
                path.pop();
            }
            _ => return Err(invalid("local directory leaves the captured revision")),
        }
    }
    if !checked_metadata(root, &path)?.is_some_and(|metadata| metadata.is_dir()) {
        return Err(invalid("local directory is missing or not a directory"));
    }
    let physical = path
        .canonicalize()
        .map_err(|_| invalid("cannot resolve local directory"))?;
    if !physical.starts_with(root) {
        return Err(invalid("local directory leaves the captured revision"));
    }
    Ok(physical)
}

fn checked_metadata(root: &Path, path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("path leaves captured revision"))?;
    if relative.as_os_str().is_empty() {
        return std::fs::symlink_metadata(root)
            .map(Some)
            .map_err(|_| invalid("cannot inspect capture root"));
    }
    let components: Vec<_> = relative.components().collect();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid("unsafe configuration path"));
        }
        current.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid("cannot inspect configuration path")),
        };
        if metadata.file_type().is_symlink() {
            return Err(invalid("symlink configuration paths are not supported"));
        }
        if index + 1 == components.len() {
            return Ok(Some(metadata));
        }
        if !metadata.is_dir() {
            return Err(invalid("non-directory configuration ancestor"));
        }
    }
    Ok(None)
}

fn anchor_environment(command: &mut Command, caller: &Path) -> Result<(), String> {
    for name in [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "HOME",
        "USERPROFILE",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            if value.is_empty() {
                return Err(invalid("empty selected home or target path is ambiguous"));
            }
            if !Path::new(&value).is_absolute() {
                command.env(name, caller.join(value));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let paths = std::env::split_paths(&path).map(|path| {
            if path.is_absolute() {
                path
            } else {
                caller.join(path)
            }
        });
        command.env(
            "PATH",
            std::env::join_paths(paths)
                .map_err(|_| invalid("cannot anchor selected executable paths"))?,
        );
    }
    if let Some(value) = std::env::var_os("RUSTUP_TOOLCHAIN") {
        let path = Path::new(&value);
        if !path.is_absolute() && path.components().count() > 1 {
            command.env("RUSTUP_TOOLCHAIN", caller.join(path));
        }
    }
    Ok(())
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self, String> {
        for _ in 0..16 {
            let path = std::env::temp_dir().join(format!(
                "rustograph-capture-cargo-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match create_private_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(invalid("cannot create private metadata context")),
            }
        }
        Err(invalid("cannot allocate a new private metadata context"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn refuse_ancestor_configuration(directory: &Path) -> Result<(), String> {
    let directory = directory
        .canonicalize()
        .map_err(|_| invalid("cannot resolve neutral metadata context"))?;
    for ancestor in directory.ancestors() {
        let cargo = ancestor.join(".cargo");
        match std::fs::symlink_metadata(&cargo) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(invalid(
                    "cannot inspect neutral Cargo ancestors; choose another TMPDIR",
                ))
            }
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(invalid(
                    "unsafe neutral Cargo ancestor; choose another TMPDIR",
                ))
            }
            Ok(_) => {}
        }
        for name in ["config", "config.toml"] {
            match std::fs::symlink_metadata(cargo.join(name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(invalid(
                        "neutral ancestor has Cargo configuration; choose another TMPDIR",
                    ))
                }
            }
        }
    }
    Ok(())
}
