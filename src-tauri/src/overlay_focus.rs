use std::process::Command;

const RULE_ID: &str = "openreader-overlay-no-focus";
const RULE: &[(&str, &str)] = &[
    (
        "Description",
        "OpenReader overlay: keep keyboard focus in source window",
    ),
    (
        "wmclass",
        "(?i)^(openreader|org[.]openreader[.]OpenReader)$",
    ),
    ("wmclassmatch", "3"),
    ("wmclasscomplete", "false"),
    ("title", "^OpenReader Overlay(:-?[0-9]+:-?[0-9]+:[0-9]+)?$"),
    ("titlematch", "3"),
    ("types", "4294967295"),
    ("acceptfocus", "false"),
    ("acceptfocusrule", "2"),
];

// GTK 3's accept-focus hint is a no-op on Wayland. KWin must enforce this
// before the first map, including the brief initialization show/hide.
pub fn ensure_kde_rule() -> Result<(), String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !desktop.to_lowercase().contains("kde") {
        return Ok(());
    }

    let mut changed = false;
    for &(key, value) in RULE {
        changed |= ensure_value(RULE_ID, key, value)?;
    }

    let existing = read_value("General", "rules")?;
    let rules = registered_rules(&existing);
    changed |= ensure_value("General", "rules", &rules)?;
    changed |= ensure_value("General", "count", &rules.split(',').count().to_string())?;
    if changed {
        run("qdbus6", &["org.kde.KWin", "/KWin", "reconfigure"])?;
        // KWin debounces reconfiguration for 200 ms. Delay first mapping when
        // installing/updating the rule, then make a compositor round trip.
        std::thread::sleep(std::time::Duration::from_millis(500));
        run(
            "qdbus6",
            &["org.kde.KWin", "/KWin", "org.freedesktop.DBus.Peer.Ping"],
        )?;
    }
    Ok(())
}

fn registered_rules(existing: &str) -> String {
    std::iter::once(RULE_ID)
        .chain(
            existing
                .split(',')
                .filter(|id| !id.is_empty() && *id != RULE_ID),
        )
        .collect::<Vec<_>>()
        .join(",")
}

fn read_value(group: &str, key: &str) -> Result<String, String> {
    run(
        "kreadconfig6",
        &["--file", "kwinrulesrc", "--group", group, "--key", key],
    )
}

fn ensure_value(group: &str, key: &str, value: &str) -> Result<bool, String> {
    if read_value(group, key)? == value {
        return Ok(false);
    }
    run(
        "kwriteconfig6",
        &[
            "--file",
            "kwinrulesrc",
            "--group",
            group,
            "--key",
            key,
            value,
        ],
    )?;
    if read_value(group, key)? != value {
        return Err(format!(
            "KWin overlay focus rule could not be saved: {group}/{key}"
        ));
    }
    Ok(true)
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("KWin overlay focus protection requires {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "KWin overlay focus protection: {program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_other_rules_and_registers_once_at_highest_priority() {
        assert_eq!(registered_rules(""), RULE_ID);
        let expected = format!("{RULE_ID},other-one,other-two");
        assert_eq!(registered_rules("other-one,other-two"), expected);
        assert_eq!(
            registered_rules(&format!("other-one,{RULE_ID},other-two")),
            expected
        );
        assert_eq!(registered_rules(&expected), expected);
    }
}
