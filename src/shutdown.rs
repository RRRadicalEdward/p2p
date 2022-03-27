use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub struct ShutdownNotifier {
    should_stop: Arc<Mutex<bool>>,
}

impl ShutdownNotifier {
    pub fn new(notify: Arc<Notify>) -> Self {
        let should_stop = Arc::new(Mutex::new(false));

        tokio::spawn({
            let should_stop = Arc::clone(&should_stop);
            async move {
                notify.notified().await;
                let mut should_stop = should_stop.lock().await;
                *should_stop = true;
            }
        });

        Self { should_stop }
    }

    pub async fn should_stop(&self) -> bool {
        *self.should_stop.lock().await
    }
}
