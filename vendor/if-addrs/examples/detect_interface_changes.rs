//! Interface change notifier example.

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "illumos"
)))]
fn main() {
    let mut if_change_notifier = if_addrs::IfChangeNotifier::new().unwrap();
    println!("Waiting for interface changes...");
    loop {
        if let Ok(details) = if_change_notifier.wait(None) {
            println!("Network interfaces changed: {:#?}", details);
        }
    }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "illumos"
))]
fn main() {
    eprintln!("Interface change API is not implemented on this platform; use get_if_addrs polling.");
}
