//! Markdown preview benchmark. Run with:
//! `cargo bench --features bench --bench markdown`

use std::time::Duration;

fn fixture(sections: usize) -> String {
    let mut text = String::with_capacity(sections * 900);
    for index in 0..sections {
        text.push_str(&format!(
            "## Section {index}: rendering pipeline\n\n\
             A paragraph with **strong**, *emphasis*, `inline code`, a [link](https://example.com),\n\
             and enough text to wrap across a couple of lines in the preview pane. \
             The quick brown fox jumps over the lazy dog.\n\n\
             - first item with `code`\n\
             - second item with **bold** text\n\
             - third item with a [link](https://example.com/{index})\n\n\
             ```rust\n\
             fn section_{index}() -> usize {{ {index} }}\n\
             let value = section_{index}();\n\
             println!(\"{{value}}\");\n\
             ```\n\n\
             | Name | Value | Note |\n\
             |:-----|------:|:-----|\n\
             | alpha | {index} | left |\n\
             | beta | {} | right |\n\n",
            index * 2
        ));
    }
    text
}

fn print_report(label: &str, report: igui_editor::bench::Report) {
    println!(
        "{label:<22} {:>8.3} ms/iter  {:>7} iters  {:>6} prims  {:>6} cmds  ({:?})",
        report.average_ms,
        report.iterations,
        report.primitives,
        report.commands,
        shorten(report.elapsed)
    );
}

fn shorten(duration: Duration) -> String {
    format!("{:.1}s", duration.as_secs_f64())
}

fn main() {
    let text = fixture(240);
    println!(
        "fixture: {} bytes, {} lines",
        text.len(),
        text.lines().count()
    );

    let parse = igui_editor::bench::markdown_parse(&text, 20);
    print_report("markdown parse", parse);
    let layout = igui_editor::bench::markdown_layout(&text, 760.0, 20);
    print_report("preview layout", layout);
    let map_build = igui_editor::bench::minimap_build(&text, 20);
    print_report("minimap build", map_build);
    let map_typing = igui_editor::bench::minimap_incremental(&text, 200);
    print_report("minimap incremental", map_typing);
    for (label, enabled) in [("typing, map off", false), ("typing, map on", true)] {
        match igui_editor::bench::editor_typing(&text, 60, 16, enabled) {
            Ok(report) => print_report(label, report),
            Err(error) => {
                eprintln!("editor typing benchmark failed: {error}");
                std::process::exit(1);
            }
        }
    }
    match igui_editor::bench::markdown_paint(&text, 120) {
        Ok(paint) => print_report("preview paint", paint),
        Err(error) => {
            eprintln!("preview paint benchmark failed: {error}");
            std::process::exit(1);
        }
    }
    match igui_editor::bench::markdown_typing(&text, 60, 16) {
        Ok(typing) => print_report("typing (16 ms frame)", typing),
        Err(error) => {
            eprintln!("typing benchmark failed: {error}");
            std::process::exit(1);
        }
    }
}
