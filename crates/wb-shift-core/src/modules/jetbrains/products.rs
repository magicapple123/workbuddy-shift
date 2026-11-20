//! 受支持产品清单：**单一来源**（配置目录前缀 + 三平台进程标识）。
//!
//! 新增一个 JetBrains 系产品只改这一张表——配置目录名判定、Windows 映像过滤、
//! macOS `.app` 匹配、Linux `<安装根>/bin/<x>` 匹配与 exe → 前缀推断全部由它派生。
//! 过去这几处各写一份字面量，补产品时必然漏一处（表现为「某平台识别不到」）。
//!
//! 与 R5-19 的决策一致：16 个产品（含已停更的 AppCode 与 Google 发行的 Android
//! Studio），不引入任何新依赖。
//!
//! Android Studio 的配置目录写在**另一个配置根**（`.../Google`）下，见
//! [`super::detect::jetbrains_config_roots`]；本模块只管「产品是谁」，不管「根在哪」。

use std::path::Path;

/// 受支持产品：配置目录前缀 + 三平台进程标识。
///
/// `windows_exes` 允许一个产品前缀对多个映像：IDEA 与 IDEA CE 的安装映像相同（社区版
/// 只能靠安装目录名或 `product-info.json` 区分），因此 [`product_prefix_from_exe_name`]
/// 取**首个**命中行（旗舰版），社区版由 `detect::fallback_config_dir_name` 再判。
pub(super) struct ProductDef {
    /// 配置目录名 / `product-info.json#dataDirectoryName` 前缀。
    pub(super) prefix: &'static str,
    /// Windows 主进程映像名（64 位在前，历史 32 位在后）。
    pub(super) windows_exes: &'static [&'static str],
    /// macOS `.app` bundle 名（不含 `.app` 后缀）。
    ///
    /// 只有 macOS 分支读它；其余平台保留字段是为了让这张表三平台完整——新增产品时
    /// 不会因为「本机看不到它被用到」而漏填某一平台。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) macos_app: &'static str,
    /// Linux `<安装根>/bin/<x>` 可执行名。
    pub(super) linux_bin: &'static str,
}

/// 16 个受支持产品，顺序即 [`product_prefix_from_exe_name`] 的优先级（旗舰版在前）。
pub(super) const PRODUCTS: [ProductDef; 16] = [
    ProductDef {
        prefix: "IntelliJIdea",
        windows_exes: &["idea64.exe", "idea.exe"],
        macos_app: "IntelliJ IDEA",
        linux_bin: "idea",
    },
    ProductDef {
        prefix: "IdeaIC",
        // 与旗舰版同一套映像：社区版只能靠安装目录名 / product-info 区分。
        windows_exes: &["idea64.exe", "idea.exe"],
        macos_app: "IntelliJ IDEA CE",
        linux_bin: "idea",
    },
    ProductDef {
        prefix: "PyCharm",
        windows_exes: &["pycharm64.exe", "pycharm.exe"],
        macos_app: "PyCharm",
        linux_bin: "pycharm",
    },
    ProductDef {
        prefix: "PyCharmCE",
        // 同上：社区版与专业版共用映像名。
        windows_exes: &["pycharm64.exe", "pycharm.exe"],
        macos_app: "PyCharm CE",
        linux_bin: "pycharm",
    },
    ProductDef {
        prefix: "WebStorm",
        windows_exes: &["webstorm64.exe", "webstorm.exe"],
        macos_app: "WebStorm",
        linux_bin: "webstorm",
    },
    ProductDef {
        prefix: "PhpStorm",
        windows_exes: &["phpstorm64.exe", "phpstorm.exe"],
        macos_app: "PhpStorm",
        linux_bin: "phpstorm",
    },
    ProductDef {
        prefix: "GoLand",
        windows_exes: &["goland64.exe", "goland.exe"],
        macos_app: "GoLand",
        linux_bin: "goland",
    },
    ProductDef {
        prefix: "CLion",
        windows_exes: &["clion64.exe", "clion.exe"],
        macos_app: "CLion",
        linux_bin: "clion",
    },
    ProductDef {
        prefix: "RubyMine",
        windows_exes: &["rubymine64.exe", "rubymine.exe"],
        macos_app: "RubyMine",
        linux_bin: "rubymine",
    },
    ProductDef {
        prefix: "DataGrip",
        windows_exes: &["datagrip64.exe", "datagrip.exe"],
        macos_app: "DataGrip",
        linux_bin: "datagrip",
    },
    ProductDef {
        prefix: "Rider",
        windows_exes: &["rider64.exe", "rider.exe"],
        macos_app: "Rider",
        linux_bin: "rider",
    },
    ProductDef {
        prefix: "Aqua",
        windows_exes: &["aqua64.exe", "aqua.exe"],
        macos_app: "Aqua",
        linux_bin: "aqua",
    },
    ProductDef {
        prefix: "DataSpell",
        windows_exes: &["dataspell64.exe", "dataspell.exe"],
        macos_app: "DataSpell",
        linux_bin: "dataspell",
    },
    ProductDef {
        // 已停更（JetBrains 2023-12 停止维护）：保留前缀与进程名只为兼容旧安装，
        // 新装用户不会命中；README / ARCHITECTURE 同样标注停更。
        prefix: "AppCode",
        windows_exes: &["appcode64.exe", "appcode.exe"],
        macos_app: "AppCode",
        linux_bin: "appcode",
    },
    ProductDef {
        prefix: "MPS",
        windows_exes: &["mps64.exe", "mps.exe"],
        macos_app: "MPS",
        linux_bin: "mps",
    },
    ProductDef {
        // Google 发行，配置目录写在 `.../Google/AndroidStudio<版本>`。
        prefix: "AndroidStudio",
        windows_exes: &["studio64.exe", "studio.exe"],
        macos_app: "Android Studio",
        linux_bin: "studio",
    },
];

/// 支持的产品前缀（由 [`PRODUCTS`] 派生，禁止手写第二份）。
pub(super) const PRODUCT_PREFIXES: [&str; PRODUCTS.len()] = {
    let mut prefixes: [&str; PRODUCTS.len()] = [""; PRODUCTS.len()];
    let mut index = 0;
    while index < PRODUCTS.len() {
        prefixes[index] = PRODUCTS[index].prefix;
        index += 1;
    }
    prefixes
};

/// 按映像名（`idea64.exe` / `idea`）查产品前缀；带不带 `.exe` 都认、大小写不敏感。
///
/// Windows 映像名与 Linux 可执行名共用一次查找：Linux 上 `/proc` 里的 argv[0] 可能带
/// 也可能不带 `.exe`，两种历史命名都要兼容。未知映像返回 `None`（不猜）。
pub(super) fn product_prefix_from_exe_name(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    PRODUCTS
        .iter()
        .find(|product| {
            product
                .windows_exes
                .iter()
                .any(|image| image.trim_end_matches(".exe").eq_ignore_ascii_case(stem))
                || product.linux_bin.eq_ignore_ascii_case(stem)
        })
        .map(|product| product.prefix)
}

/// 按可执行文件路径查产品前缀（取文件名部分）。
pub(super) fn product_prefix_from_exe(exe: &Path) -> Option<&'static str> {
    product_prefix_from_exe_name(&exe.file_name()?.to_string_lossy())
}

/// Windows 主进程映像名清单（由 [`PRODUCTS`] 派生、去重保序）。
///
/// 用来替代过去在 PowerShell 过滤、tasklist 兜底与结果回调里各写一遍的字面量列表：
/// 三处口径必须完全一致，手写副本必然漂移。
#[cfg(target_os = "windows")]
pub(super) fn windows_image_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for product in PRODUCTS.iter() {
        for image in product.windows_exes {
            if !names.iter().any(|known| known.eq_ignore_ascii_case(image)) {
                names.push(image);
            }
        }
    }
    names
}

/// Linux `<安装根>/bin/<x>` 可执行名清单（由 [`PRODUCTS`] 派生、去重保序）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_executable_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for product in PRODUCTS.iter() {
        if !names
            .iter()
            .any(|known| known.eq_ignore_ascii_case(product.linux_bin))
        {
            names.push(product.linux_bin);
        }
    }
    names
}

/// macOS `.app` bundle 下的可执行目录模式（`<App 名>.app/Contents/MacOS`）。
#[cfg(target_os = "macos")]
pub(super) fn macos_process_patterns() -> Vec<String> {
    PRODUCTS
        .iter()
        .map(|product| format!("{}.app/Contents/MacOS", product.macos_app))
        .collect()
}
