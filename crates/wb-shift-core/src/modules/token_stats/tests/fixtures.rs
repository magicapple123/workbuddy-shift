    /// 固定 fixture 的 mtime，让「原始会话先于副本被处理」的断言不依赖目录遍历顺序。
    ///
    /// Windows 上 `File::set_modified` 需要 `FILE_WRITE_ATTRIBUTES`，而 `File::open`
    /// 只申请 `GENERIC_READ`——只读句柄改时间会得到 `os error 5`，故用可写句柄打开。
    fn pin_mtime(path: std::path::PathBuf, mtime: std::time::SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open fixture for mtime")
            .set_modified(mtime)
            .expect("pin fixture mtime");
    }
