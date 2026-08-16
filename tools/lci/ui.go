package main

import (
	"fmt"
	"os"
	"strings"
	"time"
)

// ANSI escape codes
const (
	ansiReset  = "\033[0m"
	ansiBold   = "\033[1m"
	ansiRed    = "\033[31m"
	ansiGreen  = "\033[32m"
	ansiYellow = "\033[33m"
	ansiBlue   = "\033[34m"
	ansiCyan   = "\033[36m"
	ansiGray   = "\033[90m"
	ansiWhite  = "\033[37m"
)

// UI handles all terminal output with optional color support.
type UI struct {
	color bool
}

func newUI(color bool) *UI {
	return &UI{color: color}
}

func (u *UI) c(code, text string) string {
	if !u.color {
		return text
	}
	return code + text + ansiReset
}

func (u *UI) bold(text string) string   { return u.c(ansiBold, text) }
func (u *UI) red(text string) string    { return u.c(ansiRed, text) }
func (u *UI) green(text string) string  { return u.c(ansiGreen, text) }
func (u *UI) yellow(text string) string { return u.c(ansiYellow, text) }
func (u *UI) blue(text string) string   { return u.c(ansiBlue, text) }
func (u *UI) cyan(text string) string   { return u.c(ansiCyan, text) }
func (u *UI) gray(text string) string   { return u.c(ansiGray, text) }

// header prints the pipeline banner.
func (u *UI) header(root string, members, excluded int) {
	fmt.Fprintf(os.Stderr, "\n%s  Local CI\n", u.bold("lci"))
	fmt.Fprintf(os.Stderr, "%s %s\n", u.gray("workspace:"), root)
	fmt.Fprintf(os.Stderr, "%s %d members, %d test exclusions\n\n",
		u.gray("crates:"), members, excluded)
}

// stageStart logs the beginning of a stage.
func (u *UI) stageStart(name, cmd string) {
	fmt.Fprintf(os.Stderr, "  %s %s %s\n",
		u.blue(">>"),
		u.bold(padRight(name, 8)),
		u.gray(cmd))
}

// stagePass logs a successful stage.
func (u *UI) stagePass(name string, dur time.Duration) {
	fmt.Fprintf(os.Stderr, "  %s %s %s\n",
		u.green("OK"),
		padRight(name, 8),
		u.gray(formatDuration(dur)))
}

// stageFail logs a failed stage.
func (u *UI) stageFail(name string, dur time.Duration) {
	fmt.Fprintf(os.Stderr, "  %s %s %s\n",
		u.red("FAIL"),
		padRight(name, 8),
		u.gray(formatDuration(dur)))
}

// stageSkip logs a cached/skipped stage.
func (u *UI) stageSkip(name string) {
	fmt.Fprintf(os.Stderr, "  %s %s %s\n",
		u.yellow("--"),
		padRight(name, 8),
		u.gray("cached, skipping"))
}

// stageOutput prints captured command output (shown on failure).
func (u *UI) stageOutput(output string) {
	lines := strings.Split(strings.TrimRight(output, "\n"), "\n")
	max := 40 // last 40 lines
	start := 0
	if len(lines) > max {
		start = len(lines) - max
		fmt.Fprintf(os.Stderr, "     %s\n", u.gray(fmt.Sprintf("... (%d lines truncated)", start)))
	}
	for _, line := range lines[start:] {
		fmt.Fprintf(os.Stderr, "     %s\n", line)
	}
}

// summary prints the final results table.
func (u *UI) summary(results []stageResult, total time.Duration) {
	fmt.Fprintf(os.Stderr, "\n%s\n", u.gray(strings.Repeat("-", 48)))
	fmt.Fprintf(os.Stderr, "  %-10s %-8s %s\n",
		u.bold("Stage"), u.bold("Status"), u.bold("Duration"))
	fmt.Fprintf(os.Stderr, "%s\n", u.gray(strings.Repeat("-", 48)))

	allPass := true
	for _, r := range results {
		status := ""
		switch r.Status {
		case statusPass:
			status = u.green("PASS")
		case statusFail:
			status = u.red("FAIL")
			allPass = false
		case statusSkip:
			status = u.yellow("SKIP")
		}
		fmt.Fprintf(os.Stderr, "  %-10s %-18s %s\n",
			r.Name, status, u.gray(formatDuration(r.Duration)))
	}

	fmt.Fprintf(os.Stderr, "%s\n", u.gray(strings.Repeat("-", 48)))

	verdict := u.green("PASS")
	if !allPass {
		verdict = u.red("FAIL")
	}
	fmt.Fprintf(os.Stderr, "  %-10s %-18s %s\n\n",
		u.bold("Total"), verdict, u.gray(formatDuration(total)))
}

// listStages prints available stage names.
func (u *UI) listStages() {
	fmt.Println("Available stages:")
	fmt.Println("  fmt      cargo fmt --all -- --check")
	fmt.Println("  clippy   cargo clippy --workspace --all-targets -- -D warnings")
	fmt.Println("  test     cargo test --workspace --exclude <configured>")
	fmt.Println("  check    cargo check --workspace")
	fmt.Println("  build    cargo build --workspace")
}

// Logging helpers

func (u *UI) info(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", u.cyan("info"), fmt.Sprintf(format, args...))
}

func (u *UI) success(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", u.green("ok"), fmt.Sprintf(format, args...))
}

func (u *UI) warn(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", u.yellow("warn"), fmt.Sprintf(format, args...))
}

func (u *UI) fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", u.red("error"), fmt.Sprintf(format, args...))
	os.Exit(1)
}

// Env check output

func (u *UI) envOk(name string) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", u.green("found"), name)
}

func (u *UI) envMissing(name, hint string) {
	fmt.Fprintf(os.Stderr, "  %s %s  %s\n", u.red("missing"), name, u.gray(hint))
}

func (u *UI) envOptional(name, hint string) {
	fmt.Fprintf(os.Stderr, "  %s %s  %s\n", u.yellow("optional"), name, u.gray(hint))
}

// Helpers

func padRight(s string, width int) string {
	if len(s) >= width {
		return s
	}
	return s + strings.Repeat(" ", width-len(s))
}

func formatDuration(d time.Duration) string {
	if d == 0 {
		return ""
	}
	if d < time.Second {
		return fmt.Sprintf("%dms", d.Milliseconds())
	}
	if d < time.Minute {
		return fmt.Sprintf("%.1fs", d.Seconds())
	}
	m := int(d.Minutes())
	s := d.Seconds() - float64(m)*60
	return fmt.Sprintf("%dm%.1fs", m, s)
}
