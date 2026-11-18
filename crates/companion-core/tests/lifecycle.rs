use companion_core::{adapters::Collector, atomic_json, settings};
use serde_json::json;
use std::path::PathBuf;

#[path = "lifecycle/codeg.rs"]
mod codeg;
#[path = "lifecycle/codex.rs"]
mod codex;
#[path = "lifecycle/custom.rs"]
mod custom;
#[path = "lifecycle/ide.rs"]
mod ide;
#[path = "lifecycle/workbuddy.rs"]
mod workbuddy;

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        // The wall clock alone can repeat inside one process, and a duplicated
        // home lets one test's Drop delete another test's directory.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "studio-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn collector(&self, source: &str) -> Collector {
        let mut s = settings::defaults();
        for id in settings::SOURCES {
            s["sources"][id]["enabled"] = json!(id == source);
        }
        atomic_json(&self.0.join(".workbuddy-shift-companion/settings.json"), &s).unwrap();
        Collector::new(self.0.clone()).unwrap()
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
