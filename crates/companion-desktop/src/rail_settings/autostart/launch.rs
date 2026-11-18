//! 开机自启的启动目标解析（Launch）。

use super::*;

/// Login-time launch target. `args` follow `program`; `env_path` carries PATH
/// for the macOS dev fallback, which has no Windows/Linux equivalent.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) env_path: Option<String>,
}

/// What to start after login. macOS launches the bundle through `open`;
/// Windows and Linux register the packaged executable itself. A dev build is
/// unsupported there: it needs the Vite server, and neither the Run value nor
/// the desktop entry can inject the toolchain PATH, so registering one would
/// only produce a login item that never works.
pub(crate) fn launch_command(app: &tauri::AppHandle) -> Result<Launch, String> {
    // Only Linux resolves its target through the app handle.
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = bundle() {
            return Ok(Launch {
                program: "/usr/bin/open".into(),
                args: vec!["-gj".into(), bundle.to_string_lossy().into_owned()],
                env_path: None,
            });
        }
        // A dev executable needs its Vite server. Start the existing dev script,
        // retaining the Node toolchain PATH instead of registering a broken binary.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let path = std::env::var("PATH").unwrap_or_default();
        let npm = std::env::split_paths(&path)
            .map(|p| p.join("npm"))
            .find(|p| p.is_file())
            .ok_or("找不到 npm，无法配置开发版自启")?;
        if !root.join("package.json").is_file() {
            return Err("找不到桌面启动项目".into());
        }
        Ok(Launch {
            program: npm,
            args: vec![
                "--prefix".into(),
                root.to_string_lossy().into_owned(),
                "run".into(),
                "desktop:dev".into(),
            ],
            env_path: Some(path),
        })
    }
    #[cfg(target_os = "windows")]
    {
        if cfg!(debug_assertions) {
            return Err("开发版不注册开机自启，请使用打包后的应用".into());
        }
        let program = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(Launch {
            program: std::fs::canonicalize(program).map_err(|e| e.to_string())?,
            args: Vec::new(),
            env_path: None,
        })
    }
    #[cfg(target_os = "linux")]
    {
        if cfg!(debug_assertions) {
            return Err("开发版不注册开机自启，请使用打包后的应用".into());
        }
        let program = tauri::process::current_binary(&app.env()).map_err(|e| e.to_string())?;
        Ok(Launch {
            program,
            args: Vec::new(),
            env_path: None,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("当前运行环境不支持开机自启".into())
    }
}
