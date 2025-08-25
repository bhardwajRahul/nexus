//! Header rules engine for transforming HTTP headers.

use config::{HeaderRule, NameOrPattern};
use http::header::{self, HeaderMap, HeaderName};
use std::sync::OnceLock;

/// The header deny list - headers that cannot be forwarded.
static DENY_LIST: OnceLock<[HeaderName; 21]> = OnceLock::new();

/// Apply header rules to build a new header map for outgoing requests.
///
/// This function is optimized for the common case where we're building headers for downstream requests.
///
/// # Arguments
/// * `incoming_headers` - Headers from the incoming request (for forward/remove)
/// * `header_rules` - Rules to apply in order
///
/// # Returns
/// A new HeaderMap with the transformed headers.
pub fn apply(incoming_headers: &HeaderMap, header_rules: &[HeaderRule]) -> HeaderMap {
    let mut result = HeaderMap::new();

    if header_rules.is_empty() {
        return result;
    }

    for rule in header_rules {
        match rule {
            HeaderRule::Forward(forward) => apply_forward_rule(incoming_headers, forward, &mut result),
            HeaderRule::Insert(insert) => apply_insert_rule(insert, &mut result),
            HeaderRule::Remove(remove) => apply_remove_rule(remove, &mut result),
            HeaderRule::RenameDuplicate(dup) => apply_rename_duplicate_rule(incoming_headers, dup, &mut result),
        }
    }

    result
}

/// Get the header deny list.
pub fn get_deny_list() -> &'static [HeaderName] {
    DENY_LIST.get_or_init(|| {
        [
            header::ACCEPT,
            header::ACCEPT_CHARSET,
            header::ACCEPT_ENCODING,
            header::ACCEPT_RANGES,
            header::CONTENT_LENGTH,
            header::CONTENT_TYPE,
            // hop-by-hop headers
            header::CONNECTION,
            HeaderName::from_static("keep-alive"),
            header::PROXY_AUTHENTICATE,
            header::PROXY_AUTHORIZATION,
            header::TE,
            header::TRAILER,
            header::TRANSFER_ENCODING,
            header::UPGRADE,
            header::ORIGIN,
            header::HOST,
            header::SEC_WEBSOCKET_VERSION,
            header::SEC_WEBSOCKET_KEY,
            header::SEC_WEBSOCKET_ACCEPT,
            header::SEC_WEBSOCKET_PROTOCOL,
            header::SEC_WEBSOCKET_EXTENSIONS,
        ]
    })
}

/// Check if a header name is in the deny list.
pub fn is_header_denied(name: &HeaderName) -> bool {
    get_deny_list().contains(name)
}

/// Apply a forward rule to the headers.
fn apply_forward_rule(incoming_headers: &HeaderMap, forward: &config::HeaderForward, result: &mut HeaderMap) {
    match &forward.name {
        NameOrPattern::Name(header_name) => {
            if is_header_denied(header_name) {
                return;
            }

            // Remove any existing header with this name first to prevent duplication
            result.remove(header_name.as_ref());

            let value = incoming_headers
                .get(header_name.as_ref())
                .cloned()
                .or_else(|| forward.default.as_ref().map(|d| d.as_ref().clone()));

            if let Some(val) = value {
                if let Some(new_name) = &forward.rename {
                    result.insert(new_name.as_ref().clone(), val);
                } else {
                    result.insert(header_name.as_ref().clone(), val);
                }
            }
        }
        NameOrPattern::Pattern(pattern) => {
            let headers_to_forward: Vec<_> = incoming_headers
                .keys()
                .filter(|k| !is_header_denied(k) && pattern.0.is_match(k.as_str()))
                .map(|k| (k.clone(), incoming_headers.get(k).cloned().unwrap()))
                .collect();

            for (original_name, value) in headers_to_forward {
                if let Some(new_name) = &forward.rename {
                    result.insert(new_name.as_ref().clone(), value);
                } else {
                    result.insert(original_name, value);
                }
            }
        }
    }
}

/// Apply an insert rule to the headers.
fn apply_insert_rule(insert: &config::HeaderInsert, result: &mut HeaderMap) {
    result.insert(insert.name.as_ref().clone(), insert.value.as_ref().clone());
}

/// Apply a remove rule to the headers.
fn apply_remove_rule(remove: &config::HeaderRemove, result: &mut HeaderMap) {
    match &remove.name {
        NameOrPattern::Name(header_name) => {
            result.remove(header_name.as_ref());
        }
        NameOrPattern::Pattern(pattern) => {
            let to_remove: Vec<_> = result
                .keys()
                .filter(|key| pattern.0.is_match(key.as_str()))
                .cloned()
                .collect();

            for key in to_remove {
                result.remove(&key);
            }
        }
    }
}

/// Apply a rename-duplicate rule to the headers.
fn apply_rename_duplicate_rule(
    incoming_headers: &HeaderMap,
    dup: &config::HeaderRenameDuplicate,
    result: &mut HeaderMap,
) {
    let value = incoming_headers
        .get(dup.name.as_ref())
        .cloned()
        .or_else(|| dup.default.as_ref().map(|d| d.as_ref().clone()));

    if let Some(val) = value {
        result.insert(dup.name.as_ref().clone(), val.clone());
        result.insert(dup.rename.as_ref().clone(), val);
    }
}
