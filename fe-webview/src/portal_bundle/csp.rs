//! Host-authored Content-Security-Policy composer (protocol.md §5); see `portal_bundle/AGENTS.md` §csp.

use fe_format::portal_bundle::PortalCsp;

use super::mount::MountMode;

/// Policy for responses that belong to no mount (unknown label, refused before lookup).
pub const REFUSAL_POLICY: &str =
    "default-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'";

/// Longest source expression accepted from a manifest or feed.
pub(crate) const MAX_SOURCE_BYTES: usize = 255;

/// A manifest or feed origin that cannot appear in a header as one source expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsafeSource(pub String);

/// Compose the mount's policy from the validated vocabulary; feed sources count only on Static mounts.
///
/// Every source is re-checked to be one ASCII source token, so a header can never gain a directive
/// or a non-ASCII byte (wry silently drops a header whose value is not visible ASCII: SP desk check F-1).
pub fn compose(csp: &PortalCsp, mode: MountMode, feed_sources: &[String]) -> Result<String, UnsafeSource> {
    let feeds: &[String] = match mode {
        MountMode::Static => feed_sources,
        MountMode::Admitted => &[],
    };
    let mut directives: Vec<String> = Vec::with_capacity(13);
    directives.push("default-src 'none'".into());
    directives.push(with_flag("script-src 'self'", csp.wasm, "'wasm-unsafe-eval'"));
    directives.push(with_flag("style-src 'self'", csp.inline_styles, "'unsafe-inline'"));
    directives.push(with_sources("img-src 'self' data: blob:", &[csp.img.as_slice()])?);
    directives.push(with_sources("font-src 'self'", &[csp.font.as_slice()])?);
    directives.push(with_sources("media-src 'self'", &[csp.media.as_slice()])?);
    directives.push(with_sources("connect-src 'self'", &[csp.connect.as_slice(), feeds])?);
    directives.push(if csp.frame.is_empty() {
        "frame-src 'none'".into()
    } else {
        with_sources("frame-src", &[csp.frame.as_slice()])?
    });
    directives.push("worker-src 'self'".into());
    directives.push("form-action 'none'".into());
    directives.push("base-uri 'none'".into());
    directives.push("object-src 'none'".into());
    directives.push("frame-ancestors 'none'".into());
    Ok(directives.join("; "))
}

fn with_flag(base: &str, enabled: bool, keyword: &str) -> String {
    if enabled {
        format!("{base} {keyword}")
    } else {
        base.to_owned()
    }
}

fn with_sources(base: &str, lists: &[&[String]]) -> Result<String, UnsafeSource> {
    let mut out = base.to_owned();
    for source in lists.iter().flat_map(|list| list.iter()) {
        if !is_source_token(source) {
            return Err(UnsafeSource(source.clone()));
        }
        out.push(' ');
        out.push_str(source);
    }
    Ok(out)
}

/// `http(s)://` + lowercase host/port characters + an optional `[a-z0-9/_-]` path (a feed's share route): one CSP host-source.
fn is_source_token(source: &str) -> bool {
    let Some(rest) = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    !host.is_empty()
        && source.len() <= MAX_SOURCE_BYTES
        && host
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-' | b':' | b'[' | b']'))
        && path
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'/' | b'_' | b'-'))
}
