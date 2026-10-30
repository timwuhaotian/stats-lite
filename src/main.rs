//! stats-lite: CPU + disk % in the menu bar. No settings, no charts.
//!
//! The title only changes when a number changes, so an idle machine costs no
//! WindowServer redraws. It checks for updates itself: see `update`.

use std::{cell::RefCell, time::Duration};

mod update;
mod update_window;

use objc2::{
    AnyThread, ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::AnyObject, sel,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSAttributedStringAttachmentConveniences, NSFont, NSFontAttributeName,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusBarButton, NSTextAttachment,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSAttributedString, NSMutableAttributedString, NSObject, NSObjectProtocol, NSPoint, NSRange,
    NSRect, NSSize, NSString, NSTimer,
};
use sysinfo::System;

/// CPU re-sampled this often. 1s polling is what made Stats burn 59% CPU to
/// watch an idle machine; 2s still feels live.
const CPU_INTERVAL: f64 = 2.0;
/// Disk space moves by the minute at most: re-read it every 30 ticks (60s).
const DISK_EVERY_TICKS: u64 = 30;
/// Digit size: the menu bar's own title size.
const FONT_SIZE: f64 = 13.0;
/// Icon height in points: the digits' cap height, so icon and number read as one.
const ICON_HEIGHT: f64 = 13.0;
/// Nudges the icon down onto the digits' baseline; tuned against a screenshot.
const ICON_Y_OFFSET: f64 = -2.0;
/// A download's progress is re-read this often while it runs.
pub(crate) const UPDATE_POLL: Duration = Duration::from_millis(100);
/// First update check, in ticks: after launch, so it never competes with startup.
const FIRST_CHECK_TICK: u64 = 3;
/// Re-check every 6h: a menu bar stays up for days, and one wakeup in hours is enough.
const CHECK_EVERY_TICKS: u64 = 10_800;

struct Sampler {
    sys: System,
    /// Last disk reading; refreshed every [`DISK_EVERY_TICKS`] ticks.
    disk: Option<u64>,
    /// The button, `mem::forget`-ed at startup: alive for the whole process.
    button: *const NSStatusBarButton,
    /// `cpu|disk` of the last paint; the attributed title is only rebuilt on change.
    last_title: String,
    tick: u64,
    font: Retained<NSFont>,
    cpu_icon: Option<Retained<NSTextAttachment>>,
    disk_icon: Option<Retained<NSTextAttachment>>,
    /// The update menu item, `mem::forget`-ed at startup: the menu retains it.
    update_item: *const NSMenuItem,
    last_update_title: String,
    update_is_restart: bool,
}

thread_local! {
    static SAMPLER: RefCell<Option<Sampler>> = const { RefCell::new(None) };
}

/// Mounts probed in order: the writable Data volume first (the macOS 10.15+
/// split layout), then `/` (older single-volume layouts and anything else).
/// Both report the same container on 10.15+, so the order only matters where
/// the Data volume does not exist.
const DISK_MOUNTS: [&str; 2] = ["/System/Volumes/Data", "/"];

/// Used % of the boot volume, purgeable space excluded. `statfs` reports the
/// same available bytes as `df` and Storage settings on every macOS release,
/// while sysinfo's `available_space` counts purgeable space as free (about 10
/// points lower here). `None` when no mount can be read or reports 0 blocks.
fn disk_used_percent() -> Option<u64> {
    DISK_MOUNTS
        .iter()
        .find_map(|mount| statfs_used_percent(mount))
}

fn statfs_used_percent(mount: &str) -> Option<u64> {
    let path = std::ffi::CString::new(mount).ok()?;
    // SAFETY: `path` is NUL-terminated, `fs` is a valid out-pointer, and
    // `statfs` has no version-gated behaviour on macOS.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut fs) } != 0 {
        return None;
    }
    used_percent(fs.f_blocks as u64, fs.f_bavail as u64, fs.f_bsize as u64)
}

/// Share of blocks in use, `None` on a 0-sized volume. Saturating: layouts
/// that report more available than total read as 0%, never wrap around.
fn used_percent(blocks: u64, bavail: u64, bsize: u64) -> Option<u64> {
    let total = blocks.checked_mul(bsize)?;
    if total == 0 {
        return None;
    }
    let avail = bavail.checked_mul(bsize)?;
    Some(total.saturating_sub(avail) * 100 / total)
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
        let Some(sampler) = sampler.as_mut() else {
            return;
        };
        // Menu and update-window actions below all run here, on the main thread.
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        sampler.tick += 1;
        sampler.sys.refresh_cpu_usage();
        if sampler.tick % DISK_EVERY_TICKS == 0 {
            sampler.disk = disk_used_percent();
        }
        let cpu = sampler.sys.global_cpu_usage().round() as u64;
        let disk = sampler.disk;
        // Cache key: rebuilding the attributed title a tick is a WindowServer
        // redraw a tick; skip the identical ones.
        let key = match disk {
            Some(disk) => format!("{cpu}|{disk}"),
            None => format!("{cpu}|-"),
        };
        if key != sampler.last_title {
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
        }
        refresh_update_item(sampler);
        update_window::refresh(mtm);
        if sampler.tick == FIRST_CHECK_TICK || sampler.tick % CHECK_EVERY_TICKS == 0 {
            update::check_now(false);
        }
    });
}

/// The update menu item follows the background check: title, action and enabled state.
fn refresh_update_item(sampler: &mut Sampler) {
    let (title, is_restart) = update::menu_status();
    if title == sampler.last_update_title && is_restart == sampler.update_is_restart {
        return;
    }
    sampler.last_update_title = title.clone();
    sampler.update_is_restart = is_restart;
    // SAFETY: set at startup from the item the menu retains, never nulled afterwards.
    let item = unsafe { &*sampler.update_item };
    item.setTitle(&NSString::from_str(&title));
    // SAFETY: `setAction:` takes a SEL; both are implemented by `Ticker`, the target.
    let _: () = unsafe {
        msg_send![item, setAction: if is_restart { sel!(restartToUpdate:) } else { sel!(checkForUpdates:) }]
    };
}

/// A modal alert; every caller runs on the main thread already.
fn show_alert(mtm: MainThreadMarker, message: &str, info: &str) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(info));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.runModal();
}

/// Installs a staged update, then terminates; an install failure aborts the quit.
fn quit_now() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if let Some(err) = update::install_on_quit() {
        show_alert(mtm, "Couldn't Install the Update", &err);
        return;
    }
    NSApplication::sharedApplication(mtm).terminate(None);
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

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: &AnyObject) {
            update::check_now(true);
            if let Some(mtm) = MainThreadMarker::new() {
                // SAFETY: the ticker is deliberately kept alive for the process, and
                // implements the two selectors used by the update window's buttons.
                unsafe {
                    update_window::show(mtm, self as *const Self as *const AnyObject);
                }
            }
        }

        #[unsafe(method(dismissUpdate:))]
        fn dismiss_update(&self, _sender: &AnyObject) {
            update_window::dismiss();
        }

        #[unsafe(method(restartToUpdate:))]
        fn restart_to_update(&self, _sender: &AnyObject) {
            if update::restart() {
                quit_now();
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            quit_now();
        }
    }
);

fn main() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    // CPU % is a delta between two samples: take the first one before the UI exists.
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(300));
    sys.refresh_cpu_usage();

    let app = NSApplication::sharedApplication(mtm);
    // No dock icon even for an unbundled `cargo run` (the .app also sets LSUIElement).
    // SAFETY: `setActivationPolicy:` takes an NSInteger; 1 is `NSApplicationActivationPolicyAccessory`.
    let _: bool = unsafe { msg_send![&app, setActivationPolicy: 1i64] };

    // The ticker first: menu items target it, and the timer retains it. Both live
    // until the process quits, which is exactly the app's lifetime.
    // SAFETY: nothing beyond `NSObject`'s contract; no ivars, no `Drop`.
    let ticker: Retained<Ticker> = unsafe { msg_send![Ticker::alloc(), init] };
    let target = Retained::as_ptr(&ticker) as *mut AnyObject;
    std::mem::forget(ticker);

    // `NSVariableStatusItemLength`: the item hugs the title, minimal menu-bar width.
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

    // SAFETY: `setTarget:`/`setAction:` take an object and a SEL; the actions are
    // implemented by `Ticker`, the target.
    let menu = NSMenu::new(mtm);
    let update_item: Retained<NSMenuItem> = unsafe { msg_send![NSMenuItem::alloc(mtm), init] };
    let _: () = unsafe { msg_send![&update_item, setTarget: target] };
    let _: () = unsafe { msg_send![&update_item, setAction: sel!(checkForUpdates:)] };
    menu.addItem(&update_item);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit: Retained<NSMenuItem> = unsafe { msg_send![NSMenuItem::alloc(mtm), init] };
    quit.setTitle(&NSString::from_str("Quit stats-lite"));
    let _: () = unsafe { msg_send![&quit, setTarget: target] };
    let _: () = unsafe { msg_send![&quit, setAction: sel!(quit:)] };
    quit.setKeyEquivalent(&NSString::from_str("q"));
    menu.addItem(&quit);
    item.setMenu(Some(&menu));
    // The menu retains its items, and the item retains nothing back: see `on_tick`.
    let update_item_ptr = Retained::as_ptr(&update_item);
    std::mem::forget(update_item);

    let Some(button) = item.button(mtm) else {
        return;
    };
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
            disk: disk_used_percent(),
            button: button_ptr,
            last_title: String::new(),
            tick: 0,
            font,
            cpu_icon,
            disk_icon,
            update_item: update_item_ptr,
            last_update_title: String::new(),
            update_is_restart: false,
        });
    });
    on_tick();

    // SAFETY: the class method retains the repeating timer; the target above outlives
    // it. The block-free target/selector form is used because these bindings generate
    // no `scheduledTimerWithTimeInterval:repeats:block:`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_percent_matches_df() {
        // This machine: statfs/df report 235_016_896_512 available of
        // 494_384_795_648 total, i.e. 52% used.
        assert_eq!(used_percent(494_384_795_648, 235_016_896_512, 1), Some(52));
    }

    #[test]
    fn used_percent_ignores_nothing_but_blocks() {
        assert_eq!(used_percent(100, 48, 4096), Some(52));
        assert_eq!(used_percent(100, 100, 4096), Some(0));
        // More available than total saturates to 0% instead of wrapping.
        assert_eq!(used_percent(100, 200, 4096), Some(0));
    }

    #[test]
    fn used_percent_rejects_empty_volumes() {
        assert_eq!(used_percent(0, 0, 4096), None);
        assert_eq!(used_percent(100, 50, 0), None);
    }

    #[test]
    fn important_usage_capacity_would_read_low() {
        // sysinfo's `available_space` counts purgeable space as free
        // (294_976_599_106 here), which is how the menu bar showed 40%
        // while `df` showed ~50%. Pin the gap so it can't regress silently.
        assert_eq!(used_percent(494_384_795_648, 294_976_599_106, 1), Some(40));
    }
}
