//! Owned remote metadata with Git configuration byte preservation.

use std::collections::BTreeMap;

use gix::bstr::{BString, ByteSlice};
use interoptopus::ffi;

use crate::{GixError, chain_to_string};

/// An owned remote name and its fetch/push URL lists, in configuration order.
#[ffi]
#[derive(Debug, Clone)]
pub struct RemoteRecord {
    pub name: ffi::Vec<u8>,
    pub fetch_urls: ffi::Vec<ffi::Vec<u8>>,
    pub push_urls: ffi::Vec<ffi::Vec<u8>>,
}

type Urls = Vec<Vec<u8>>;

fn append_urls(target: &mut Urls, values: Vec<BString>) {
    for value in values {
        // Git uses an empty URL to reset values inherited from earlier sections.
        if value.is_empty() {
            target.clear();
        } else {
            target.push(value.into());
        }
    }
}

fn record(name: BString, fetch: Urls, push: Urls) -> RemoteRecord {
    RemoteRecord {
        name: ffi::Vec::from(Vec::<u8>::from(name)),
        fetch_urls: ffi::Vec::from(fetch.into_iter().map(ffi::Vec::from).collect::<Vec<_>>()),
        push_urls: ffi::Vec::from(push.into_iter().map(ffi::Vec::from).collect::<Vec<_>>()),
    }
}

pub(crate) fn remotes(repo: &mut gix::Repository, resolve_urls: bool) -> Result<Vec<RemoteRecord>, GixError> {
    if resolve_urls {
        repo.reload().map_err(GixError::from)?;
        return repo.remote_names().into_iter().map(|name| {
            let remote = repo.try_find_remote(name.as_bstr())
                .ok_or_else(|| GixError::Config(crate::message("configured remote disappeared from its snapshot")))?
                .map_err(|err| GixError::Config(chain_to_string(&err)))?;
            let urls = |direction| remote.urls(direction)
                .map(|url| Vec::<u8>::from(url.to_bstring())).collect::<Urls>();
            Ok(record(name, urls(gix::remote::Direction::Fetch), urls(gix::remote::Direction::Push)))
        }).collect();
    }

    let config = repo.config_snapshot().reload().map_err(|err| GixError::Config(chain_to_string(&err)))?;
    let mut remotes: BTreeMap<BString, (Urls, Urls)> = BTreeMap::new();
    if let Some(sections) = config.sections_by_name("remote") {
        for section in sections {
            // Repo's current open/create entry points use gix's default section trust policy.
            if !gix::config::section::is_trusted(section.meta()) {
                continue;
            }
            if let Some(name) = section.header().subsection_name() {
                let (fetch, push) = remotes.entry(name.to_owned()).or_default();
                append_urls(fetch, section.values("url"));
                append_urls(push, section.values("pushurl"));
            }
        }
    }
    Ok(remotes.into_iter().map(|(name, (fetch, push))| {
        let push = if push.is_empty() { fetch.clone() } else { push };
        record(name, fetch, push)
    }).collect())
}