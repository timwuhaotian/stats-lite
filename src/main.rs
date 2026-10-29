//! stats-lite: CPU + disk % in the menu bar. No settings, no charts.
//!
//! The title only changes when a number changes, so an idle machine costs no
//! WindowServer redraws.

use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAttributedStringAttachmentConveniences, NSFont, NSFontAttributeName,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusBarButton, NSTextAttachment,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSAttributedString, NSMutableAttributedString, NSObject, NSObjectProtocol, NSPoint, NSRange,
    NSRect, NSSize, NSString, NSTimer,
};
use sysinfo::{Disks, System};

/// CPU re-sampled this often. 1s polling is what made Stats burn 59% CPU to
/// watch an idle machine; 2s still feels live.
const CPU_INTERVAL: f64 = 2.0;
/// Disk space moves by the minute at most: re-list it every 30 ticks (60s).
const DISK_EVERY_TICKS: u64 = 30;
/// Digit size: the menu bar's own title size.
const FONT_SIZE: f64 = 13.0;
/// Icon height in points: the digits' cap height, so icon and number read as one.
const ICON_HEIGHT: f64 = 13.0;
/// Nudges the icon down onto the digits' baseline; tuned against a screenshot.
const ICON_Y_OFFSET: f64 = -2.0;

struct Sampler {
    sys: System,
    disks: Disks,
    /// The button, `mem::forget`-ed at startup: alive for the whole process.
    button: *const NSStatusBarButton,
    /// `cpu|disk` of the last paint; the attributed title is only rebuilt on change.
    last_title: String,
    tick: u64,
    font: Retained<NSFont>,
    cpu_icon: Option<Retained<NSTextAttachment>>,
    disk_icon: Option<Retained<NSTextAttachment>>,
}

thread_local! {
    static SAMPLER: RefCell<Option<Sampler>> = const { RefCell::new(None) };
}

/// Used % of the `/` volume. `None` when it can't be found or reports 0 total.
fn disk_used_percent(disks: &Disks) -> Option<u64> {
    let root = disks
        .list()
        .iter()
        .find(|disk| disk.mount_point() == std::path::Path::new("/"))?;
    let total = root.total_space();
    if total == 0 {
        return None;
    }
    Some((total - root.available_space()) * 100 / total)
}

/// An SF Symbol as a menu-bar template icon, scaled to [`ICON_HEIGHT`]. `None`
/// when the symbol doesn't exist (no macOS ships without these two, in practice).
fn make_icon(name: &str, description: &str) -> Option<Retained<NSTextAttachment>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )?;
    // Monochrome like every other menu-bar icon, in light and dark mode.
    image.setTemplate(true);
    let size = image.size();
    let scale = if size.height > 0.0 {
        ICON_HEIGHT / size.height
    } else {
        1.0
    };
    let attachment = NSTextAttachment::new();
    attachment.setImage(Some(&image));
    attachment.setBounds(NSRect::new(
        NSPoint::new(0.0, ICON_Y_OFFSET),
        NSSize::new(size.width * scale, ICON_HEIGHT),
    ));
    Some(attachment)
}

fn text_run(s: &str) -> Retained<NSAttributedString> {
    NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(s))
}

fn icon_run(icon: Option<&NSTextAttachment>, fallback: &str) -> Retained<NSAttributedString> {
    match icon {
        Some(icon) => NSAttributedString::attributedStringWithAttachment(icon),
        None => text_run(fallback),
    }
}

fn on_tick() {
    SAMPLER.with(|sampler| {
        let mut sampler = sampler.borrow_mut();
        let Some(sampler) = sampler.as_mut() else { return };
        sampler.tick += 1;
        sampler.sys.refresh_cpu_usage();
        if sampler.tick % DISK_EVERY_TICKS == 0 {
            sampler.disks = Disks::new_with_refreshed_list();
        }
        let cpu = sampler.sys.global_cpu_usage().round() as u64;
        let disk = disk_used_percent(&sampler.disks);
        // Cache key: rebuilding the attributed title a tick is a WindowServer
        // redraw a tick; skip the identical ones.
        let key = match disk {
            Some(disk) => format!("{cpu}|{disk}"),
            None => format!("{cpu}|-"),
        };
        if key == sampler.last_title {
            return;
        }
        sampler.last_title = key;

        // Monospaced digits, right-aligned to width 3: 5% -> 100% never bounces
        // the neighbouring menu-bar icons around.
        let title = NSMutableAttributedString::new();
        title.appendAttributedString(&icon_run(sampler.cpu_icon.as_deref(), "CPU"));
        title.appendAttributedString(&text_run(&format!(" {cpu:>3}%  ")));
        title.appendAttributedString(&icon_run(sampler.disk_icon.as_deref(), "D"));
        let disk_text = match disk {
            Some(disk) => format!(" {disk:>3}%"),
            // Em dash, never 0%: a missing volume must not read as empty.
            None => String::from(" \u{2014}"),
        };
        title.appendAttributedString(&text_run(&disk_text));
        // SAFETY: `NSFontAttributeName` takes an `NSFont`, which is what's passed.
        unsafe {
            title.addAttribute_value_range(
                NSFontAttributeName,
                &sampler.font,
                NSRange::new(0, title.length()),
            );
        }
        // SAFETY: set at startup from the button that was `mem::forget`-ed
        // (the status item retains its button), never nulled afterwards.
        let button = unsafe { &*sampler.button };
        button.setAttributedTitle(&title);
        // Labels are icons now, so the tooltip (and VoiceOver) carries the words.
        let tip = match disk {
            Some(disk) => format!("stats-lite: CPU {cpu}%, disk {disk}%"),
            None => format!("stats-lite: CPU {cpu}%, disk unreadable"),
        };
        let tip = NSString::from_str(&tip);
        let _: () = unsafe { msg_send![button, setToolTip: &*tip] };
    });
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements; no ivars, no `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "StatsLiteTicker"]
    struct Ticker;

    // SAFETY: nothing beyond the protocol's own contract.
    unsafe impl NSObjectProtocol for Ticker {}

    impl Ticker {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            on_tick();
        }
    }
);

fn main() {
    let Some(mtm) = MainThreadMarker::new() else { return };

    // CPU % is a delta between two samples: take the first one before the UI exists.
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(300));
    sys.refresh_cpu_usage();

    let app = NSApplication::sharedApplication(mtm);
    // No dock icon even for an unbundled `cargo run` (the .app also sets LSUIElement).
    // SAFETY: `setActivationPolicy:` takes an NSInteger; 1 is `NSApplicationActivationPolicyAccessory`.
    let _: bool = unsafe { msg_send![&app, setActivationPolicy: 1i64] };

    // `NSVariableStatusItemLength`: the item hugs the title, minimal menu-bar width.
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

    // The only UI besides the numbers: a Quit item. No target: `terminate:` walks the
    // responder chain up to NSApplication on its own.
    let menu = NSMenu::new(mtm);
    let quit: Retained<NSMenuItem> = unsafe { msg_send![NSMenuItem::alloc(mtm), init] };
    quit.setTitle(&NSString::from_str("Quit stats-lite"));
    // SAFETY: `setAction:` takes a SEL; `terminate:` is implemented by `NSApplication`,
    // which the responder chain reaches because the item has no target.
    let _: () = unsafe { msg_send![&quit, setAction: sel!(terminate:)] };
    quit.setKeyEquivalent(&NSString::from_str("q"));
    menu.addItem(&quit);
    item.setMenu(Some(&menu));

    let Some(button) = item.button(mtm) else { return };
    // The status item retains its button, and neither is freed: see `on_tick`.
    let button_ptr = Retained::as_ptr(&button);
    std::mem::forget(button);
    std::mem::forget(item);

    // 0.0 is `NSFontWeightRegular`; the constant costs one more feature for nothing.
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, 0.0);
    let cpu_icon = make_icon("cpu", "CPU usage");
    let disk_icon = make_icon("internaldrive", "disk usage");

    SAMPLER.with(|sampler| {
        *sampler.borrow_mut() = Some(Sampler {
            sys,
            disks: Disks::new_with_refreshed_list(),
            button: button_ptr,
            last_title: String::new(),
            tick: 0,
            font,
            cpu_icon,
            disk_icon,
        });
    });
    on_tick();

    // SAFETY: the class method retains the repeating timer, and the timer retains its
    // target; both live until the process quits, which is exactly the app's lifetime.
    // The block-free target/selector form is used because these bindings generate no
    // `scheduledTimerWithTimeInterval:repeats:block:`.
    let ticker: Retained<Ticker> = unsafe { msg_send![Ticker::alloc(), init] };
    let target = Retained::as_ptr(&ticker) as *mut AnyObject;
    std::mem::forget(ticker);
    let user_info: Option<&NSObject> = None;
    let _timer: Retained<NSTimer> = unsafe {
        msg_send![
            NSTimer::class(),
            scheduledTimerWithTimeInterval: CPU_INTERVAL,
            target: target,
            selector: sel!(tick:),
            userInfo: user_info,
            repeats: true,
        ]
    };
    app.run();
}
