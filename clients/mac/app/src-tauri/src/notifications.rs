use std::collections::{HashSet, VecDeque};

use macbot_desktop_services::local_settings::NotificationPreferences;
use serde_json::Value;

pub fn diagnostics_enabled() -> bool {
    std::env::var_os("MACBOT_NOTIFICATION_DIAGNOSTICS").is_some()
}

pub fn diagnostic(message: &str) {
    if !diagnostics_enabled() {
        return;
    }
    use std::io::Write;
    let path = std::env::var_os("MACBOT_NOTIFICATION_LOG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp/macbot-notifications.log".into());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    NeedsYou,
    Review,
    DirectMessage,
    GroupMessage,
}

pub fn copy(key: &str) -> &'static str {
    match key {
        "notify.need_you.title" => "需要你 · {bot}",
        "notify.approval.body" => "审批：{summary}",
        "notify.review.title" => "待验收 · 总管",
        "notify.review.body" => "「{project}」做完了，请你验收",
        _ => "",
    }
}

pub fn classify_message(
    message: &Value,
    chat: &Value,
    preferences: &NotificationPreferences,
) -> Option<NotificationKind> {
    if message["sender"]["kind"] == "user" || chat["muted"] == true {
        return None;
    }

    let blocks = message["blocks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let needs_you = blocks.iter().any(|block| {
        matches!(
            block["type"].as_str(),
            Some("question" | "approval" | "takeover_request" | "blocked" | "loop_paused")
        )
    }) || message["delivery"]
        .as_array()
        .is_some_and(|delivery| delivery.iter().any(|item| item["state"] == "queued"));
    if needs_you && preferences.need_you {
        return Some(NotificationKind::NeedsYou);
    }

    let review = blocks
        .iter()
        .any(|block| block["type"] == "review_card" && block["state"] == "pending");
    if review && preferences.review {
        return Some(NotificationKind::Review);
    }

    let chat_kind = chat["kind"].as_str().unwrap_or_default();
    let is_dm = matches!(chat_kind, "direct" | "bot_dm" | "main");
    let is_group = chat_kind == "project";
    let mentions_user = message["mentions"]
        .as_array()
        .is_some_and(|mentions| mentions.iter().any(|mention| mention["kind"] == "user"));
    if (is_dm || (is_group && mentions_user)) && preferences.dm_mention {
        return Some(NotificationKind::DirectMessage);
    }
    if is_group && preferences.all_group {
        return Some(NotificationKind::GroupMessage);
    }
    None
}

pub fn stable_notification_id(message: &Value) -> Option<String> {
    if let Some(id) = message.get("id").and_then(Value::as_str) {
        if id.starts_with("approval:") {
            return Some(id.to_owned());
        }
    }
    let approval_id = message
        .get("blocks")?
        .as_array()?
        .iter()
        .find_map(|block| block.get("approval_id").and_then(Value::as_str));
    if let Some(approval_id) = approval_id {
        return Some(format!("approval:{approval_id}"));
    }
    message.get("id")?.as_str().map(str::to_owned)
}

#[derive(Default)]
pub struct RecentNotificationIds {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl RecentNotificationIds {
    pub fn insert(&mut self, host_id: &str, message_id: &str) -> bool {
        if host_id.is_empty() || message_id.is_empty() {
            return false;
        }
        let key = format!("{host_id}:{message_id}");
        if !self.ids.insert(key.clone()) {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > 4096 {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

pub fn should_suppress_for_current_chat(
    application_active: bool,
    active_chat_id: Option<&str>,
    message_chat_id: &str,
) -> bool {
    application_active && active_chat_id == Some(message_chat_id)
}

#[cfg(target_os = "macos")]
pub fn application_is_active() -> bool {
    use objc2_app_kit::NSRunningApplication;

    NSRunningApplication::currentApplication().isActive()
}

#[cfg(target_os = "macos")]
pub fn install_delegate() -> Result<(), String> {
    native::install_delegate()
}

#[cfg(not(target_os = "macos"))]
pub fn application_is_active() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod native {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::NSArray;
    use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationSettings,
        UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = ()]
        struct MacBotNotificationDelegate;

        unsafe impl NSObjectProtocol for MacBotNotificationDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for MacBotNotificationDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present_notification(
                &self,
                _center: &UNUserNotificationCenter,
                notification: &UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                let identifier = notification.request().identifier();
                super::diagnostic(&format!(
                    "willPresent identifier={identifier} options=banner,list,sound"
                ));
                completion_handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }
        }
    );

    impl MacBotNotificationDelegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: NSObject's init is the designated initializer for this stateless delegate.
            unsafe { msg_send![super(this), init] }
        }
    }

    static DELEGATE_INSTALLED: AtomicBool = AtomicBool::new(false);

    pub fn install_delegate() -> Result<(), String> {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return Err("系统通知只在应用程序包中可用".into());
        }
        if DELEGATE_INSTALLED.load(Ordering::Acquire) {
            return Ok(());
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = MacBotNotificationDelegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // UNUserNotificationCenter keeps its delegate weakly, so retain it for
        // the process lifetime.
        std::mem::forget(delegate);
        DELEGATE_INSTALLED.store(true, Ordering::Release);
        super::diagnostic("delegate installed");
        Ok(())
    }

    pub fn show(
        title: &str,
        body: &str,
        identifier: &str,
        sound_enabled: bool,
    ) -> Result<(), String> {
        install_delegate()?;
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::new();
        let title = NSString::from_str(title);
        let body = NSString::from_str(body);
        let request_id = identifier.to_owned();
        let identifier = NSString::from_str(identifier);
        content.setTitle(&title);
        content.setBody(&body);
        if sound_enabled {
            let sound_name = NSString::from_str("BubblePop.caf");
            let sound = UNNotificationSound::soundNamed(&sound_name);
            content.setSound(Some(&sound));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            None,
        );
        let delivery_center = center.clone();
        let scheduled_id = request_id.clone();
        if super::diagnostics_enabled() {
            let settings_handler =
                RcBlock::new(|settings: std::ptr::NonNull<UNNotificationSettings>| {
                    let settings = unsafe { settings.as_ref() };
                    super::diagnostic(&format!(
                        "settings status={} alert={} sound={} notification_center={}",
                        settings.authorizationStatus().0,
                        settings.alertSetting().0,
                        settings.soundSetting().0,
                        settings.notificationCenterSetting().0,
                    ));
                });
            center.getNotificationSettingsWithCompletionHandler(&settings_handler);
        }
        let handler = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if let Some(error) = unsafe { error.as_ref() } {
                super::diagnostic(&format!(
                    "authorization error={}",
                    error.localizedDescription()
                ));
            } else if granted.as_bool() {
                let add_id = request_id.clone();
                let delivered_center = delivery_center.clone();
                let add_handler = RcBlock::new(move |error: *mut NSError| {
                    if let Some(error) = unsafe { error.as_ref() } {
                        super::diagnostic(&format!(
                            "add identifier={add_id} error={}",
                            error.localizedDescription()
                        ));
                    } else {
                        super::diagnostic(&format!("add identifier={add_id} error=none"));
                        if super::diagnostics_enabled() {
                            let delivered_id = add_id.clone();
                            let delivered_handler = RcBlock::new(
                                move |notifications: std::ptr::NonNull<NSArray<UNNotification>>| {
                                    let notifications = unsafe { notifications.as_ref() };
                                    let identifiers = notifications
                                        .iter()
                                        .map(|notification| {
                                            notification.request().identifier().to_string()
                                        })
                                        .collect::<Vec<_>>();
                                    super::diagnostic(&format!(
                                        "delivered query count={} matching={} identifiers={identifiers:?}",
                                        identifiers.len(),
                                        identifiers.iter().any(|id| id == &delivered_id),
                                    ));
                                },
                            );
                            delivered_center
                                .getDeliveredNotificationsWithCompletionHandler(&delivered_handler);
                        }
                    }
                });
                delivery_center
                    .addNotificationRequest_withCompletionHandler(&request, Some(&add_handler));
            } else {
                super::diagnostic("authorization denied");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &handler,
        );
        super::diagnostic(&format!("scheduled identifier={scheduled_id}"));
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn show_native_with_sound(
    title: &str,
    body: &str,
    identifier: &str,
    sound_enabled: bool,
) -> Result<(), String> {
    native::show(title, body, identifier, sound_enabled)
}

#[cfg(target_os = "macos")]
pub fn play_bubble(app: &tauri::AppHandle) -> Result<(), String> {
    use std::{path::Path, process::Command};
    use tauri::Manager;

    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|resources| resources.join("BubblePop.caf"))
        .filter(|path| path.is_file());
    let development = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/macbot-desktop/assets/Sounds/BubblePop.caf");
    let sound = bundled
        .or_else(|| development.is_file().then_some(development))
        .ok_or_else(|| "找不到提示音资源".to_string())?;
    Command::new("/usr/bin/afplay")
        .arg(sound)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("无法播放提示音：{error}"))?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn play_bubble(_app: &tauri::AppHandle) -> Result<(), String> {
    Err("提示音仅支持 macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn show_native_with_sound(
    _title: &str,
    _body: &str,
    _identifier: &str,
    _sound_enabled: bool,
) -> Result<(), String> {
    Err("系统通知仅支持 macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> NotificationPreferences {
        NotificationPreferences::default()
    }

    #[test]
    fn classifies_need_you_review_and_message_preferences() {
        let preferences = defaults();
        let approval = serde_json::json!({"sender":{"kind":"bot"},"blocks":[{"type":"approval"}]});
        assert_eq!(
            classify_message(&approval, &serde_json::json!({"kind":"main"}), &preferences),
            Some(NotificationKind::NeedsYou)
        );

        let review = serde_json::json!({"sender":{"kind":"bot"},"blocks":[{"type":"review_card","state":"pending"}]});
        assert_eq!(
            classify_message(
                &review,
                &serde_json::json!({"kind":"project"}),
                &preferences
            ),
            Some(NotificationKind::Review)
        );

        let dm = serde_json::json!({"sender":{"kind":"bot"}});
        assert_eq!(
            classify_message(&dm, &serde_json::json!({"kind":"direct"}), &preferences),
            Some(NotificationKind::DirectMessage)
        );

        let mentioned_group =
            serde_json::json!({"sender":{"kind":"bot"},"mentions":[{"kind":"user"}]});
        assert_eq!(
            classify_message(
                &mentioned_group,
                &serde_json::json!({"kind":"project"}),
                &preferences
            ),
            Some(NotificationKind::DirectMessage)
        );

        let group = serde_json::json!({"sender":{"kind":"bot"},"blocks":[{"type":"text"}]});
        assert_eq!(
            classify_message(&group, &serde_json::json!({"kind":"project"}), &preferences),
            None
        );
    }

    #[test]
    fn honors_category_global_and_muted_chat_preferences() {
        let mut preferences = defaults();
        let dm = serde_json::json!({"sender":{"kind":"bot"},"mentions":[{"kind":"user"}]});
        preferences.dm_mention = false;
        assert_eq!(
            classify_message(&dm, &serde_json::json!({"kind":"direct"}), &preferences),
            None
        );
        let mentioned_group =
            serde_json::json!({"sender":{"kind":"bot"},"mentions":[{"kind":"user"}]});
        assert_eq!(
            classify_message(
                &mentioned_group,
                &serde_json::json!({"kind":"project"}),
                &preferences
            ),
            None
        );
        let user = serde_json::json!({"sender":{"kind":"user"},"blocks":[{"type":"approval"}]});
        assert_eq!(
            classify_message(&user, &serde_json::json!({"kind":"main"}), &defaults()),
            None
        );
        assert_eq!(
            classify_message(
                &dm,
                &serde_json::json!({"kind":"direct","muted":true}),
                &defaults()
            ),
            None
        );
    }

    #[test]
    fn suppresses_current_chat_when_app_is_active_and_deduplicates_ids() {
        assert!(should_suppress_for_current_chat(
            true,
            Some("chat-a"),
            "chat-a"
        ));
        assert!(!should_suppress_for_current_chat(
            true,
            Some("chat-a"),
            "chat-b"
        ));
        assert!(!should_suppress_for_current_chat(
            false,
            Some("chat-a"),
            "chat-a"
        ));
        let mut ids = RecentNotificationIds::default();
        assert!(ids.insert("host-a", "message-1"));
        assert!(!ids.insert("host-a", "message-1"));
        assert!(ids.insert("host-b", "message-1"));
        assert!(!ids.insert("", "message-2"));
        let approval_message = serde_json::json!({"id":"msg-approval","blocks":[{"type":"approval","approval_id":"apr-1"}]});
        let approval_event = serde_json::json!({"id":"approval:apr-1","blocks":[{"type":"approval","approval_id":"apr-1"}]});
        assert_eq!(
            stable_notification_id(&approval_message),
            stable_notification_id(&approval_event)
        );
    }
}
