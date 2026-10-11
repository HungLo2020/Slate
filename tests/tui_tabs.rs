//! Crowded tab bars in the terminal interface, driven through a real
//! pseudo-terminal: a narrow terminal pane numbers its tabs, and the tab
//! keys switch terminals without the keys reaching the shell.
mod support;
use support::Env;

const F6: &[u8] = b"\x1b[17~";
const F7: &[u8] = b"\x1b[18~";
const SHIFT_F7: &[u8] = b"\x1b[18;2~";
const F8: &[u8] = b"\x1b[19~";
const CTRL_PAGE_UP: &[u8] = b"\x1b[5;5~";
const CTRL_PAGE_DOWN: &[u8] = b"\x1b[6;5~";

#[test]
fn a_narrow_terminal_pane_keeps_every_terminal_reachable() {
    let env = Env::new();
    let root = env.path("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut tui = env.slate(&[root.to_str().unwrap()]);
    tui.wait_for("terminal #3");
    // Focus the side terminal pane, then open two more terminals in it.
    for _ in 0..4 {
        if tui.screen().contains("Shift-drag selects") {
            break;
        }
        tui.send(F6);
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    tui.wait_for("Shift-drag selects");
    tui.send("echo MARK-$((0+1))\r");
    tui.wait_for("MARK-1");
    for n in 2..=3 {
        tui.send(F8);
        tui.wait_gone(&format!("MARK-{}", n - 1));
        tui.send(format!("echo MARK-$((0+{n}))\r"));
        tui.wait_for(&format!("MARK-{n}"));
    }
    // All three tabs are numbered in the header; the hint names the keys.
    let screen = tui.screen();
    let header = screen
        .lines()
        .find(|line| line.contains(" 3:") || line.contains("‹"))
        .unwrap_or_else(|| panic!("no numbered terminal tabs:\n{screen}"));
    assert!(header.contains(" 1 ") && header.contains(" 2 "), "{screen}");
    tui.wait_for("F7/Shift+F7 tabs");
    // Back with Shift+F7 and Ctrl+PageUp, forward with F7 and Ctrl+PageDown.
    tui.send(SHIFT_F7);
    tui.wait_for("MARK-2");
    tui.send(CTRL_PAGE_UP);
    tui.wait_for("MARK-1");
    tui.send(F7);
    tui.wait_for("MARK-2");
    tui.send(CTRL_PAGE_DOWN);
    tui.wait_for("MARK-3");
    // The keys switched tabs only: no stray input reached the shells.
    assert!(!tui.screen().contains("~"), "{}", tui.screen());
}
