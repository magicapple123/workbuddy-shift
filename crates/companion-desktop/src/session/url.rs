//! 会话链接解析：`/api/open-session` 查询串与各客户端自定义 scheme。

use std::path::PathBuf;

pub(super) const VSCODE_BUNDLE: &str = "com.microsoft.VSCode";

#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) enum SessionTarget {
    Link(url::Url),
    App(&'static str),
    /// A CodeBuddy VS Code plugin session. The plugin has no URI handler, so the
    /// only addressable thing is the window's folder, which is resolved from the
    /// plugin's own history; `cwd` is a candidate, never the answer by itself.
    VSCode {
        session: String,
        cwd: Option<PathBuf>,
    },
}
fn codebuddy_international(edition: Option<&str>) -> bool {
    !matches!(edition, Some("domestic" | "codebuddycn"))
}
fn codebuddy_bundle(edition: Option<&str>) -> &'static str {
    if codebuddy_international(edition) {
        "com.tencent.codebuddy"
    } else {
        "com.tencent.codebuddycn"
    }
}
#[cfg(target_os = "macos")]
pub(super) fn app_for_scheme(scheme: &str) -> Option<(&'static str, &'static str)> {
    Some(match scheme {
        "workbuddy-ai" => ("WorkBuddy AI.app", "com.workbuddy.workbuddy-ai"),
        "workbuddy" => ("WorkBuddy.app", "com.tencent.workbuddy.mac"),
        "codebuddy" => ("CodeBuddy.app", "com.tencent.codebuddy"),
        "codebuddycn" => ("CodeBuddy CN.app", "com.tencent.codebuddycn"),
        _ => return None,
    })
}
/// The app to activate for a bare `scheme://file` link, when the scheme has one.
/// VS Code is not here: `vscode://file/...` is handled by VS Code's URL handler,
/// which always targets the last active window, so the plugin's sessions open
/// the folder by path instead (see `resolve_vscode_folder`).
fn bare_file_app(scheme: &str) -> Option<&'static str> {
    Some(match scheme {
        "codebuddy" => "com.tencent.codebuddy",
        "codebuddycn" => "com.tencent.codebuddycn",
        _ => return None,
    })
}
pub(super) fn session_target(url: &str) -> Result<SessionTarget, String> {
    if url.starts_with("/api/open-session?") {
        let parsed =
            url::Url::parse(&format!("http://localhost{url}")).map_err(|e| e.to_string())?;
        let sources: Vec<_> = parsed
            .query_pairs()
            .filter(|(key, _)| key == "source")
            .map(|(_, value)| value.into_owned())
            .collect();
        let edition = parsed
            .query_pairs()
            .find(|(key, _)| key == "edition")
            .map(|(_, value)| value.into_owned());
        let host = parsed
            .query_pairs()
            .find(|(key, _)| key == "host")
            .map(|(_, value)| value.into_owned());
        let session = parsed
            .query_pairs()
            .find(|(key, _)| key == "session")
            .map(|(_, value)| value.into_owned());
        let cwd = parsed
            .query_pairs()
            .find(|(key, _)| key == "cwd")
            .map(|(_, value)| PathBuf::from(value.into_owned()));
        return match sources.as_slice() {
            [source] if source == "codeg" => Ok(SessionTarget::App("app.codeg")),
            // Only the plugin's own target consults `host`; an unknown value
            // keeps the edition behaviour, and so does every other source.
            [source] if source == "codebuddy-ide" => {
                if host.as_deref() != Some("vscode") {
                    return Ok(SessionTarget::App(codebuddy_bundle(edition.as_deref())));
                }
                Ok(
                    match session
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| usable_session_id(id))
                    {
                        Some(session) => SessionTarget::VSCode {
                            session: session.to_string(),
                            cwd,
                        },
                        // Without a session id there is nothing to resolve a folder
                        // from, so only the application itself can be activated.
                        None => SessionTarget::App(VSCODE_BUNDLE),
                    },
                )
            }
            _ => Err("不支持的应用".into()),
        };
    }
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    let supported = matches!(
        (parsed.scheme(), parsed.host_str()),
        ("codex", Some("threads"))
            | ("workbuddy", Some("chat"))
            | ("workbuddy-ai", Some("chat"))
            | ("codebuddy", Some("file"))
            | ("codebuddycn", Some("file"))
            | ("codeg", Some("session"))
    );
    // Older frontends emitted a bare file URL when a Hook reported cwd="/".
    // Activate the app rather than opening the filesystem root as a project.
    if parsed.host_str() == Some("file")
        && matches!(parsed.path(), "" | "/")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
    {
        if let Some(app) = bare_file_app(parsed.scheme()) {
            return Ok(SessionTarget::App(app));
        }
    }
    if !supported
        || parsed.path().len() < 2
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("不支持的会话链接".into());
    }
    Ok(SessionTarget::Link(parsed))
}
/// A plugin conversation id is a plain token. Anything with a path separator or
/// a parent-directory segment would let the read-only history lookup walk out of
/// its root, so such an id is never used.
pub(super) fn usable_session_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains('/') && !id.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn link(value: &str) -> url::Url {
        match session_target(value).unwrap() {
            SessionTarget::Link(url) => url,
            _ => panic!("expected session link"),
        }
    }
    #[test]
    fn codebuddy_missing_workspace_activates_app_without_opening_root() {
        for value in [
            "/api/open-session?source=codebuddy-ide&edition=domestic",
            "codebuddycn://file/",
            "codebuddycn://file",
        ] {
            assert!(matches!(
                session_target(value).unwrap(),
                SessionTarget::App("com.tencent.codebuddycn")
            ));
        }
        for value in [
            "/api/open-session?source=codebuddy-ide",
            "/api/open-session?source=codebuddy-ide&edition=international",
            "codebuddy://file/",
            "codebuddy://file",
        ] {
            assert!(matches!(
                session_target(value).unwrap(),
                SessionTarget::App("com.tencent.codebuddy")
            ));
        }
        assert_eq!(
            link("codebuddycn://file/Users/apple/project").path(),
            "/Users/apple/project"
        );
        assert_eq!(
            link("codebuddy://file/Users/apple/project").path(),
            "/Users/apple/project"
        );
        for value in [
            "codebuddycn://user@file/",
            "codebuddy://user@file/",
            "codebuddycn://other/path",
            "/api/open-session?source=other",
            "/api/open-session?source=codeg&source=codebuddy-ide",
        ] {
            assert!(session_target(value).is_err());
        }
    }
    #[test]
    fn vscode_plugin_sessions_open_their_folder_or_the_app() {
        // `host=vscode` with a session id hands the folder resolution to the
        // desktop side; the hook's cwd rides along as a candidate, not an answer.
        match session_target(
            "/api/open-session?source=codebuddy-ide&host=vscode&session=conv-1&cwd=%2Fwork%2Fapp",
        )
        .unwrap()
        {
            SessionTarget::VSCode { session, cwd } => {
                assert_eq!(session, "conv-1");
                assert_eq!(cwd.as_deref(), Some(Path::new("/work/app")));
            }
            other => panic!("expected a VS Code target, got {other:?}"),
        }
        assert!(matches!(
            session_target("/api/open-session?source=codebuddy-ide&host=vscode&session=conv-1")
                .unwrap(),
            SessionTarget::VSCode { .. }
        ));
        // Nothing to resolve a folder from: the application is the whole action.
        for value in [
            "/api/open-session?source=codebuddy-ide&host=vscode",
            "/api/open-session?source=codebuddy-ide&host=vscode&session=",
            "/api/open-session?source=codebuddy-ide&host=vscode&session=%2E%2E%2Fother",
        ] {
            assert!(matches!(
                session_target(value).unwrap(),
                SessionTarget::App("com.microsoft.VSCode")
            ));
        }
        // `vscode://` is gone: the URL handler always uses the last active window.
        for value in [
            "vscode://file/Users/apple/project",
            "vscode://file",
            "vscode://file/",
            "vscode://other/x",
            "vscode://user@file/x",
            "vscode://file?query=1",
            "vscode://",
        ] {
            assert!(session_target(value).is_err());
        }
        // An unknown host value keeps the edition behaviour instead of guessing.
        assert!(matches!(
            session_target("/api/open-session?source=codebuddy-ide&host=cursor").unwrap(),
            SessionTarget::App("com.tencent.codebuddy")
        ));
        for (value, app) in [
            (
                "/api/open-session?source=codebuddy-ide&host=vscode&edition=domestic",
                "com.microsoft.VSCode",
            ),
            (
                "/api/open-session?source=codebuddy-ide&host=vscode",
                "com.microsoft.VSCode",
            ),
            (
                "/api/open-session?source=codebuddy-ide&host=cursor&edition=domestic",
                "com.tencent.codebuddycn",
            ),
            (
                "/api/open-session?source=codebuddy-ide&host=codebuddy-ide",
                "com.tencent.codebuddy",
            ),
        ] {
            assert!(
                matches!(session_target(value).unwrap(), SessionTarget::App(found) if found == app)
            );
        }
        assert!(session_target("/api/open-session?source=codeg&host=vscode").is_ok());
    }
    #[test]
    fn codeg_links_target_the_requested_session() {
        let target = link("codeg://session/214");
        assert_eq!(target.as_str(), "codeg://session/214");
        let encoded = link("codeg://session/task%2Fa%20%3F%23");
        assert_eq!(encoded.path(), "/task%2Fa%20%3F%23");
        for invalid in [
            "codeg://session/",
            "codeg://other/214",
            "codeg://user@session/214",
        ] {
            assert!(session_target(invalid).is_err());
        }
    }
    #[test]
    fn workbuddy_editions_use_distinct_url_schemes() {
        let domestic = link("workbuddy://chat/abc");
        assert_eq!(domestic.scheme(), "workbuddy");
        let international = link("workbuddy-ai://chat/abc");
        assert_eq!(international.scheme(), "workbuddy-ai");
        assert!(session_target("workbuddy://other/abc").is_err());
    }
}
