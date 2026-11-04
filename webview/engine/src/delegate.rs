//! Ce que le moteur annonce pour un onglet, traduit en événements de la couche navigateur.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use servo::{LoadStatus, WebView, WebViewDelegate};
use url::Url;
use webview_core::TabId;

use crate::EngineEvent;

#[derive(Debug)]
pub(crate) struct TabDelegate {
    pub(crate) tab: TabId,
    /// Posé par la notification de frame, lu par `Engine::pump`. Le delegate ne peint jamais :
    /// voir la note de D5, peindre ici consommerait la frame sale que l'embeddeur voulait rendre.
    pub(crate) dirty: Rc<Cell<bool>>,
    pub(crate) events: Rc<RefCell<VecDeque<EngineEvent>>>,
}

impl TabDelegate {
    fn push(&self, event: EngineEvent) {
        self.events.borrow_mut().push_back(event);
    }
}

impl WebViewDelegate for TabDelegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.dirty.set(true);
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        self.push(EngineEvent::UrlChanged {
            tab: self.tab,
            url: url.to_string(),
        });
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.push(EngineEvent::TitleChanged {
            tab: self.tab,
            title: title.unwrap_or_default(),
        });
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.push(EngineEvent::LoadStatusChanged {
            tab: self.tab,
            complete: status == LoadStatus::Complete,
        });
    }

    fn notify_closed(&self, _webview: WebView) {
        self.push(EngineEvent::Closed { tab: self.tab });
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        self.push(EngineEvent::Crashed {
            tab: self.tab,
            reason,
        });
    }
}
