//! System notifications through the `UNUserNotificationCenter` API.
//!
//! Everything here runs lazily: nothing touches the notification center (and
//! so nothing can trigger the authorization prompt or the framework's
//! not-in-a-bundle abort) until the application posts a notification or
//! registers a response callback.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use block2::RcBlock;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    ForegroundExecutor, SharedString, SystemNotification, SystemNotificationAction,
    SystemNotificationResponse, SystemNotificationSound, Task,
};
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationAttachment, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNAuthorizationStatus, UNNotificationRequest, UNNotificationResponse, UNNotificationSettings,
    UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

type ResponseCallback = Rc<RefCell<Option<Box<dyn FnMut(SystemNotificationResponse)>>>>;

pub(crate) struct SystemNotificationState {
    initialized: bool,
    center: Option<NotificationCenter>,
    callback: ResponseCallback,
    _response_task: Option<Task<()>>,
}

impl SystemNotificationState {
    pub(crate) fn new() -> Self {
        Self {
            initialized: false,
            center: None,
            callback: Rc::new(RefCell::new(None)),
            _response_task: None,
        }
    }

    pub(crate) fn show(&mut self, executor: &ForegroundExecutor, notification: SystemNotification) {
        self.initialize(executor);
        if let Some(center) = &self.center {
            center.show(notification);
        }
    }

    pub(crate) fn dismiss(&mut self, executor: &ForegroundExecutor, tag: &str) {
        self.initialize(executor);
        if let Some(center) = &self.center {
            center.dismiss(tag);
        }
    }

    /// `Some(false)` without an app bundle or once authorization is denied.
    pub(crate) fn allowed(&mut self, executor: &ForegroundExecutor) -> Option<bool> {
        self.initialize(executor);
        match &self.center {
            Some(center) => center.allowed(),
            None => Some(false),
        }
    }

    pub(crate) fn on_response(
        &mut self,
        executor: &ForegroundExecutor,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        self.initialize(executor);
        *self.callback.borrow_mut() = Some(callback);
    }

    fn initialize(&mut self, executor: &ForegroundExecutor) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        let (sender, mut receiver) = mpsc::unbounded();
        self.center = NotificationCenter::new(sender);
        if self.center.is_none() {
            return;
        }

        // Responses arrive from the delegate on an arbitrary thread; this task
        // hands them to the registered callback on the main thread.
        let callback = self.callback.clone();
        self._response_task = Some(executor.spawn(async move {
            while let Some(response) = receiver.next().await {
                // Take the callback out for the call: it may re-enter the
                // platform (e.g. to dismiss the notification it was told
                // about) or replace itself.
                let taken = callback.borrow_mut().take();
                if let Some(mut taken) = taken {
                    taken(response);
                    callback.borrow_mut().get_or_insert(taken);
                }
            }
        }));
    }
}

struct NotificationCenter {
    center: Retained<UNUserNotificationCenter>,
    /// The center's `delegate` property is weak; keeping the delegate retained
    /// here keeps response handling alive for the app's lifetime.
    _delegate: Retained<NotificationResponseDelegate>,
    /// The union of every action set registered so far. Categories are shared
    /// by notifications with identical actions because macOS replaces the
    /// entire registered set whenever a category is added.
    categories: RefCell<
        HashMap<Vec<SystemNotificationAction>, (SharedString, Retained<UNNotificationCategory>)>,
    >,
    authorization_requested: Cell<bool>,
    /// The last authorization status read: `ALLOWED_*`.
    allowed: Arc<AtomicU8>,
}

const ALLOWED_UNKNOWN: u8 = 0;
const ALLOWED_YES: u8 = 1;
const ALLOWED_NO: u8 = 2;

impl NotificationCenter {
    fn new(sender: mpsc::UnboundedSender<SystemNotificationResponse>) -> Option<Self> {
        // `UNUserNotificationCenter` raises `NSInternalInconsistencyException`
        // ("bundleProxyForCurrentProcess is nil"), aborting the process, when
        // the binary isn't part of an app bundle — which is how dev builds run
        // via `cargo run`. A bundle identifier is only present when launched
        // from a real `.app`, so use it as the guard.
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            log::info!("system notifications disabled: not running from an app bundle");
            return None;
        }

        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = NotificationResponseDelegate::new(sender);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let this = Self {
            center,
            _delegate: delegate,
            categories: RefCell::new(HashMap::new()),
            authorization_requested: Cell::new(false),
            allowed: Arc::new(AtomicU8::new(ALLOWED_UNKNOWN)),
        };
        this.refresh_allowed();
        Some(this)
    }

    /// The status last read (not determined counts as allowed: posting asks),
    /// and a fresh read for next time, as the person may change it any time.
    fn allowed(&self) -> Option<bool> {
        let known = match self.allowed.load(Ordering::Relaxed) {
            ALLOWED_YES => Some(true),
            ALLOWED_NO => Some(false),
            _ => None,
        };
        self.refresh_allowed();
        known
    }

    fn refresh_allowed(&self) {
        let allowed = self.allowed.clone();
        let completion = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
            // SAFETY: the center passes a valid settings object for the
            // duration of the callback.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let value = if status == UNAuthorizationStatus::Denied { ALLOWED_NO } else { ALLOWED_YES };
            allowed.store(value, Ordering::Relaxed);
        });
        self.center.getNotificationSettingsWithCompletionHandler(&completion);
    }

    fn request_authorization(&self) {
        if self.authorization_requested.replace(true) {
            return;
        }
        let allowed = self.allowed.clone();
        let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            allowed.store(if granted.as_bool() { ALLOWED_YES } else { ALLOWED_NO }, Ordering::Relaxed);
            // SAFETY: when non-null, `error` is a valid `NSError` for the
            // duration of the callback.
            if let Some(error) = unsafe { error.as_ref() } {
                log::warn!(
                    "system notification authorization failed: {}",
                    error.localizedDescription()
                );
            } else if !granted.as_bool() {
                log::info!("system notification authorization denied");
            }
        });
        self.center
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &completion,
            );
    }

    fn show(&self, notification: SystemNotification) {
        self.request_authorization();

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        if !notification.subtitle.is_empty() {
            content.setSubtitle(&NSString::from_str(&notification.subtitle));
        }
        // Without a sound the notification is silent.
        match &notification.sound {
            SystemNotificationSound::Silent => {}
            SystemNotificationSound::Default => {
                content.setSound(Some(&UNNotificationSound::defaultSound()));
            }
            SystemNotificationSound::Named(name) => {
                content.setSound(Some(&UNNotificationSound::soundNamed(&NSString::from_str(name))));
            }
        }
        if let Some(image) = notification.image.as_deref().and_then(attachment) {
            content.setAttachments(&NSArray::from_retained_slice(&[image]));
        }
        if !notification.actions.is_empty() {
            let category_identifier = self.register_category(&notification.actions);
            content.setCategoryIdentifier(&NSString::from_str(&category_identifier));
        }

        // A nil trigger delivers immediately. Reusing the tag as the request
        // identifier makes a newer notification for the same tag replace the
        // older one.
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.tag),
            &content,
            None,
        );
        let completion = RcBlock::new(|error: *mut NSError| {
            // SAFETY: when non-null, `error` is a valid `NSError` for the
            // duration of the callback.
            if let Some(error) = unsafe { error.as_ref() } {
                log::warn!(
                    "failed to deliver system notification: {}",
                    error.localizedDescription()
                );
            }
        });
        self.center
            .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
    }

    fn register_category(&self, actions: &[SystemNotificationAction]) -> SharedString {
        let mut categories = self.categories.borrow_mut();
        if let Some((identifier, _category)) = categories.get(actions) {
            return identifier.clone();
        }

        let identifier =
            SharedString::from(format!("gpui-system-notification-{}", categories.len()));
        let platform_actions: Vec<Retained<UNNotificationAction>> = actions
            .iter()
            .map(|action| {
                UNNotificationAction::actionWithIdentifier_title_options(
                    &NSString::from_str(&action.id),
                    &NSString::from_str(&action.label),
                    UNNotificationActionOptions::empty(),
                )
            })
            .collect();
        let category =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(&identifier),
                &NSArray::from_retained_slice(&platform_actions),
                &NSArray::new(),
                UNNotificationCategoryOptions::empty(),
            );

        categories.insert(actions.to_vec(), (identifier.clone(), category));
        let all: Vec<Retained<UNNotificationCategory>> = categories
            .values()
            .map(|(_identifier, category)| category.clone())
            .collect();
        self.center
            .setNotificationCategories(&NSSet::from_retained_slice(&all));
        identifier
    }

    fn dismiss(&self, tag: &str) {
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(tag)]);
        self.center
            .removePendingNotificationRequestsWithIdentifiers(&identifiers);
        self.center
            .removeDeliveredNotificationsWithIdentifiers(&identifiers);
    }
}

/// An image file as an attachment. The notification center moves the file
/// into its own store once the request is added.
fn attachment(path: &std::path::Path) -> Option<Retained<UNNotificationAttachment>> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    // SAFETY: no options dictionary is passed.
    match unsafe {
        UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
            &NSString::from_str("image"),
            &url,
            None,
        )
    } {
        Ok(attachment) => Some(attachment),
        Err(error) => {
            log::warn!(
                "system notification image {} not attached: {}",
                path.display(),
                error.localizedDescription()
            );
            None
        }
    }
}

struct DelegateIvars {
    sender: mpsc::UnboundedSender<SystemNotificationResponse>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements and
    // `NotificationResponseDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    struct NotificationResponseDelegate;

    unsafe impl NSObjectProtocol for NotificationResponseDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationResponseDelegate {
        // Called when the user activates a delivered notification, possibly on
        // a non-main thread; the channel hop hands the response to the
        // foreground task holding the receiver.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_notification_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let tag = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier();
            // The other well-known identifier, `UNNotificationDismissActionIdentifier`,
            // is only delivered for categories opting into dismiss callbacks,
            // which we never request.
            let action_id = if &*action == unsafe { UNNotificationDefaultActionIdentifier } {
                None
            } else {
                Some(SharedString::from(action.to_string()))
            };
            self.ivars()
                .sender
                .unbounded_send(SystemNotificationResponse {
                    tag: SharedString::from(tag),
                    action_id,
                })
                .ok();
            completion_handler.call(());
        }

        // Without this, macOS suppresses banners while the app is frontmost.
        // Posting is the application's decision; whether the app is focused is
        // its criterion to apply, not the platform's.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
        }
    }
);

impl NotificationResponseDelegate {
    fn new(sender: mpsc::UnboundedSender<SystemNotificationResponse>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { sender });
        // SAFETY: `NSObject`'s `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
