use std::sync::{Arc, Mutex};

use crate::daemon::control::{ControlEvent, ControlServerMsg};

use super::Sink;

#[derive(Default)]
pub struct AttachRegistry {
    guis: Mutex<Vec<GuiLive>>,
}

struct GuiLive {
    conn: u64,
    sink: Arc<Sink>,
}

impl AttachRegistry {
    pub(super) fn register_gui(&self, conn: u64, sink: Arc<Sink>) {
        let mut guis = self.guis.lock().unwrap_or_else(|e| e.into_inner());
        guis.retain(|gui| gui.conn != conn);
        guis.push(GuiLive { conn, sink });
    }

    pub(super) fn unregister_gui(&self, conn: u64) {
        self.guis
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|gui| gui.conn != conn);
    }

    pub(super) fn open_gui(&self, path: Option<String>) -> bool {
        // The newest GUI connection belongs to the most recently started app
        // process. Window recency is resolved inside that process, where GPUI
        // owns the authoritative focus state.
        loop {
            let target = self
                .guis
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last()
                .map(|gui| (gui.conn, Arc::clone(&gui.sink)));
            let Some((conn, sink)) = target else {
                return false;
            };
            let event = ControlServerMsg::Event(ControlEvent::GuiOpen { path: path.clone() });
            if sink.send(&event).is_ok() {
                return true;
            }
            self.unregister_gui(conn);
        }
    }
}
