use cargo_toml::{Manifest, Value};
use futures::future::{BoxFuture, FutureExt};
use tokio::runtime::Builder;

use std::env::{args, current_dir};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::{fs, io};

fn main() {
    let mut iter = args().peekable();
    if !iter.next().is_some_and(|x| {
        x == "cargo-nuget" || x.ends_with("/cargo-nuget") || x.ends_with("\\cargo-nuget")
    }) && iter.peek().map(|x| x.as_str()) != Some("nuget")
    {
        let mut child = Command::new("cargo")
            .args(iter)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let code = child.wait().unwrap();
        exit(code.code().unwrap_or(-1));
    }
    if iter.peek().map(|x| x.as_str()) == Some("nuget") {
        iter.next();
    }
    match iter.next().as_deref() {
        // TODO: Respect NO_COLOR
        // TODO: Propagate through all packages
        Some("install") => {
            #[cfg(unix)]
            {
                eprintln!(
                    "\x1b[1;33mwarning: \x1b[37mnuget does nothing on *nix. If you've accidentally invoked cargo-nuget in production, make sure to fix this.\x1b[0m"
                );
            }
            if let Err(why) = (Install {}).perform() {
                match why {
                    Error::NoWorkspaceRoot => {
                        eprintln!("\x1b[1;31merror: \x1b[37mcould not find workspace.\x1b[0m");
                    }
                    Error::NoCargoToml => {
                        eprintln!("\x1b[1;31merror: \x1b[37mcould not find Cargo.toml.\x1b[0m");
                    }
                    Error::MalformedManifest => {
                        eprintln!(
                            "\x1b[1;31merror: \x1b[37mCargo.toml is of an incorrect format.\x1b[0m"
                        );
                    }
                    // TODO: this error message sucks ass
                    Error::Download(error) => {
                        eprintln!("\x1b[1;31merror: \x1b[37mdownload failed.\x1b[0m\n\n{error:#}");
                    }
                    Error::Io(error) => {
                        eprintln!("\x1b[1;31mio error: {error:#}\x1b[0m");
                    }
                    Error::Other(error) => {
                        eprintln!("\x1b[1;31m{error:#}\x1b[0m");
                    }
                }
                exit(1);
            }
        }
        Some(x) => {
            eprintln!("\x1b[1;31merror: \x1b[0mno such command: `{x}`.");
            exit(1);
        }
        None => {
            eprintln!("Nugget.");
            eprintln!();
            eprintln!("\x1b[1;32mUsage: \x1b[1;36mcargo-nuget <COMMAND>");
            eprintln!();
            eprintln!("\x1b[1;32mCommands:");
            eprintln!("  \x1b[1;36minstall  \x1b[0mInstall nuget packages");
        }
    }
}

#[derive(Debug)]
enum Error {
    NoWorkspaceRoot,
    MalformedManifest,
    NoCargoToml,
    Download(Box<dyn std::error::Error>),
    Io(io::Error),
    Other(Box<dyn std::error::Error>),
}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug)]
pub struct Install {}

impl Install {
    fn perform(&self) -> Result<(), Error> {
        let root = workspace_root()?;
        let bytes = std::fs::read(root.join("Cargo.toml")).map_err(|_| Error::NoCargoToml)?;
        let manifest = Manifest::from_slice(&bytes).map_err(|_| Error::MalformedManifest)?;
        let deps = get_deps(manifest)?;
        let downloaded_deps = download_dependencies(deps)?;
        for dep in downloaded_deps {
            let dep_directory = root.join("target").join("nuget").join(&dep.dependency.name);
            // create the dependency directory
            std::fs::create_dir_all(&dep_directory).unwrap();
            for winmd in dep.winmds() {
                winmd.write(&dep_directory).unwrap();
            }

            for dll in dep.dlls() {
                dll.write(&dep_directory).unwrap();
            }
        }

        Ok(())
    }
}

fn workspace_root() -> Result<PathBuf, Error> {
    // TODO: improve it again
    current_dir()
        .unwrap()
        .ancestors()
        .find(|x| fs::File::open(x.join("Cargo.toml")).is_ok())
        .map(|x| x.to_path_buf())
        .ok_or(Error::NoWorkspaceRoot)
}

fn get_deps(manifest: Manifest) -> Result<Vec<Dependency>, Error> {
    let metadata = manifest.package.and_then(|p| p.metadata);
    match metadata {
        Some(Value::Table(mut t)) => {
            let deps = match t.remove("nuget_dependencies") {
                Some(Value::Table(deps)) => deps,
                _ => {
                    eprintln!("\x1b[1;33mwarning: \x1b[37mno nuget packages are defined.\x1b[0m");
                    eprintln!();
                    eprintln!("\x1b[1;38;5;39mnote: \x1b[37madd them to Cargo.toml:.\x1b[0m");
                    eprintln!("\x1b[1;38;5;39m  | [package.metadata.nuget_dependencies]\x1b[0m");
                    eprintln!("\x1b[1;38;5;39m  | \"Win2D.uwp\" = \"1.25.0\"\x1b[0m");
                    return Ok(vec![]);
                }
            };
            deps.into_iter()
                .map(|(key, value)| match value {
                    Value::String(version) => Ok(Dependency::new(key, version)),
                    _ => Err(Error::MalformedManifest),
                })
                .collect()
        }
        _ => {
            eprintln!("\x1b[1;33mwarning: \x1b[37mno nuget packages are defined.\x1b[0m");
            eprintln!();
            eprintln!("\x1b[1;38;5;33mnote: \x1b[37madd them to Cargo.toml:.\x1b[0m");
            eprintln!("\x1b[1;38;5;33m  | [package.metadata.nuget_dependencies]\x1b[0m");
            eprintln!("\x1b[1;38;5;33m  | \"Win2D.uwp\" = \"1.25.0\"\x1b[0m");
            Ok(vec![])
        }
    }
}

#[derive(Debug)]
struct Dependency {
    name: String,
    version: String,
}

impl Dependency {
    fn new(name: String, version: String) -> Self {
        Self { name, version }
    }

    fn url(&self) -> String {
        format!(
            "https://www.nuget.org/api/v2/package/{}/{}",
            self.name, self.version
        )
    }

    async fn download(&self) -> Result<Vec<u8>, Error> {
        fn try_download(
            url: String,
            recursion_amount: u8,
        ) -> BoxFuture<'static, Result<Vec<u8>, Error>> {
            async move {
                if recursion_amount == 0 {
                    return Err(Error::Download("Too many redirects".into()));
                }
                let res = reqwest::get(&url)
                    .await
                    .map_err(|e| Error::Download(e.into()))?;
                match res.status().into() {
                    200u16 => {
                        let bytes = res.bytes().await.map_err(|e| Error::Download(e.into()))?;
                        Ok(bytes.into_iter().collect())
                    }
                    302 => {
                        let headers = res.headers();
                        let redirect_url = headers.get("Location").unwrap();

                        let url = redirect_url.to_str().unwrap();

                        try_download(url.to_owned(), recursion_amount - 1).await
                    }
                    _ => Err(Error::Download(
                        format!("Non-successful response: {}", res.status()).into(),
                    )),
                }
            }
            .boxed()
        }

        try_download(self.url(), 5).await
    }
}

struct DownloadedDependency {
    dependency: Dependency,
    contents: (Vec<Winmd>, Vec<Dll>),
}

impl DownloadedDependency {
    fn new(dependency: Dependency, bytes: Vec<u8>) -> Result<Self, Error> {
        let contents = Self::read_contents(&bytes)?;
        Ok(Self {
            dependency,
            contents,
        })
    }

    fn winmds(&self) -> &[Winmd] {
        &self.contents.0
    }

    fn dlls(&self) -> &[Dll] {
        &self.contents.1
    }

    fn read_contents(zip: &[u8]) -> Result<(Vec<Winmd>, Vec<Dll>), Error> {
        let reader = std::io::Cursor::new(zip);
        let mut zip = zip::ZipArchive::new(reader).map_err(|e| Error::Other(Box::new(e)))?;
        let mut winmds = Vec::new();
        let mut dlls = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let path = file.enclosed_name().unwrap();
            match path.extension() {
                Some(e)
                    if e == "winmd"
                        && path.parent().and_then(Path::to_str) == Some("lib\\uap10.0") =>
                {
                    let name = path.file_name().unwrap().to_owned();
                    let mut contents = Vec::with_capacity(file.size() as usize);

                    if let Err(e) = file.read_to_end(&mut contents) {
                        eprintln!("Could not read winmd file: {e:?}");
                        continue;
                    }
                    winmds.push(Winmd { name, contents });
                }
                Some(e) if e == "dll" && path.starts_with("runtimes") => {
                    let name: PathBuf = path
                        .components()
                        .filter(|c| match c {
                            std::path::Component::Normal(p) => *p != "native" && *p != "runtimes",
                            _ => panic!("Unexpected component"),
                        })
                        .collect();
                    let mut contents = Vec::with_capacity(file.size() as usize);

                    if let Err(e) = file.read_to_end(&mut contents) {
                        eprintln!("Could not read dll: {e:?}");
                        continue;
                    }
                    dlls.push(Dll { name, contents });
                }
                _ => {}
            }
        }
        Ok((winmds, dlls))
    }
}

fn download_dependencies(deps: Vec<Dependency>) -> Result<Vec<DownloadedDependency>, Error> {
    Builder::new_current_thread()
        .enable_io()
        .build()
        .unwrap()
        .block_on(async {
            let results = deps.into_iter().map(|dep| async move {
                let bytes = dep.download().await?;
                DownloadedDependency::new(dep, bytes)
            });

            futures::future::try_join_all(results).await
        })
}

struct Winmd {
    name: OsString,
    contents: Vec<u8>,
}

impl Winmd {
    fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::write(dir.join(&self.name), &self.contents)
    }
}

struct Dll {
    name: PathBuf,
    contents: Vec<u8>,
}

impl Dll {
    fn write(&self, dir: &Path) -> Result<(), Error> {
        let path = dir.join(&self.name);

        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, &self.contents)?;
        }
        for profile in &["debug", "release"] {
            let profile_path = workspace_root()?.join("target").join(profile);
            std::fs::create_dir_all(&profile_path)?;
            let arch = self.name.parent().unwrap();
            let dll_path = profile_path.join(self.name.strip_prefix(arch).unwrap());
            if arch.as_os_str() == ARCH && std::fs::read_link(&dll_path).is_err() {
                #[cfg(windows)]
                std::os::windows::fs::symlink_file(&path, dll_path)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&path, dll_path)?;
            }
        }

        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "win10-x64";

#[cfg(target_arch = "x86")]
const ARCH: &str = "win10-x86";

#[cfg(target_arch = "arm")]
const ARCH: &str = "win10-arm";

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "win10-arm64";
