// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `AppHost` (DT§4.4's `Host`): the one trait Swift implements, for what
//! Rust must ask the OS to do — post a notification, open a URL, read a
//! Keychain item. Named apart from Foundation's `Host`, which every Swift
//! file imports. Separate from the exports so its contract is one short
//! file; tests use an in-memory host, never the real Keychain (A49).

use std::sync::Arc;

use cox_app::{Browser, BrowserError, InboxItem, PageText};

use crate::types::BrowserFailure;

/// Implemented in Swift (CoxPlatform).
#[uniffi::export(with_foreign)]
#[async_trait::async_trait]
pub trait AppHost: Send + Sync {
    /// A new inbox item arrived; `badge` is the count that blocks a turn.
    fn notify(&self, item: InboxItem, badge: u32);
    /// The badge fell with no new item: an approval or question was
    /// answered, or its session closed.
    fn badge(&self, badge: u32);
    /// An MCP server's login page, or a link the person asked to follow.
    fn open_url(&self, url: String);
    /// T52.20: a web link a session on the ssh host `origin` asked to
    /// show; opened only after the person confirms it.
    fn confirm_open_url(&self, origin: String, url: String);
    /// The Keychain secret stored for a provider section (`anthropic`,
    /// `openai`, a `[providers.<name>]`); its env var, when set, wins.
    fn secret(&self, section: String) -> Option<String>;
    /// T51.8: whether the app has a page the agent may drive (the browser
    /// pane); without one, sessions get no `browser_*` tools.
    fn has_browser(&self) -> bool;
    /// Loads `url`, which Rust already checked is `http`/`https`.
    async fn browser_load(&self, url: String) -> Result<(), BrowserFailure>;
    /// The page's title, address and visible text.
    async fn browser_text(&self) -> Result<PageText, BrowserFailure>;
    /// The visible page as PNG bytes.
    async fn browser_snapshot(&self) -> Result<Vec<u8>, BrowserFailure>;
}

/// The Swift host as `cox_app::app::Host`, which takes borrowed strings.
pub(crate) struct Bridge(pub Arc<dyn AppHost>);

impl cox_app::app::Host for Bridge {
    fn notify(&self, item: InboxItem, badge: u32) {
        self.0.notify(item, badge);
    }
    fn badge(&self, badge: u32) {
        self.0.badge(badge);
    }
    fn open_url(&self, url: &str) {
        self.0.open_url(url.to_string());
    }
    fn confirm_open_url(&self, origin: &str, url: &str) {
        self.0.confirm_open_url(origin.to_string(), url.to_string());
    }
    fn secret(&self, section: &str) -> Option<String> {
        self.0.secret(section.to_string())
    }
    fn browser(&self) -> Option<Arc<dyn Browser>> {
        self.0.has_browser().then(|| PageBridge::shared(&self.0))
    }
}

/// The Swift host's browser as `cox_app::Browser`, which takes a borrowed
/// URL and `cox-app`'s error.
pub(crate) struct PageBridge(Arc<dyn AppHost>);

impl PageBridge {
    fn shared(host: &Arc<dyn AppHost>) -> Arc<dyn Browser> {
        Arc::new(Self(Arc::clone(host)))
    }
}

#[async_trait::async_trait]
impl Browser for PageBridge {
    async fn load(&self, url: &str) -> Result<(), BrowserError> {
        self.0
            .browser_load(url.to_string())
            .await
            .map_err(Into::into)
    }
    async fn text(&self) -> Result<PageText, BrowserError> {
        self.0.browser_text().await.map_err(Into::into)
    }
    async fn snapshot(&self) -> Result<Vec<u8>, BrowserError> {
        self.0.browser_snapshot().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_app::app::Host as _;
    use std::sync::Mutex;

    /// A host with or without a page; remembers what it was asked to load.
    #[derive(Default)]
    struct PageHost {
        page: Option<PageText>,
        loaded: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AppHost for PageHost {
        fn notify(&self, _: InboxItem, _: u32) {}
        fn badge(&self, _: u32) {}
        fn open_url(&self, _: String) {}
        fn confirm_open_url(&self, _: String, _: String) {}
        fn secret(&self, _: String) -> Option<String> {
            None
        }
        fn has_browser(&self) -> bool {
            self.page.is_some()
        }
        async fn browser_load(&self, url: String) -> Result<(), BrowserFailure> {
            self.loaded.lock().expect("loaded").push(url);
            Ok(())
        }
        async fn browser_text(&self) -> Result<PageText, BrowserFailure> {
            self.page.clone().ok_or(BrowserFailure::NoPage)
        }
        async fn browser_snapshot(&self) -> Result<Vec<u8>, BrowserFailure> {
            Err(BrowserFailure::Page {
                message: "blank".into(),
            })
        }
    }

    #[tokio::test]
    async fn the_bridge_offers_a_browser_only_when_the_host_has_one() {
        assert!(Bridge(Arc::new(PageHost::default())).browser().is_none());

        let page = PageText {
            title: "Docs".into(),
            url: "http://localhost:3000/".into(),
            text: "hello".into(),
        };
        let host = Arc::new(PageHost {
            page: Some(page.clone()),
            ..PageHost::default()
        });
        let bridge = Bridge(Arc::clone(&host) as Arc<dyn AppHost>);
        let browser = bridge.browser().expect("the host has a page");
        browser.load("http://localhost:3000/").await.expect("load");
        assert_eq!(
            host.loaded.lock().expect("loaded").clone(),
            ["http://localhost:3000/"]
        );
        assert_eq!(browser.text().await, Ok(page));
        assert_eq!(
            browser.snapshot().await,
            Err(BrowserError::Page("blank".into()))
        );
    }
}
