// lci — Local CI runner for Rust workspaces.
//
// Mirrors the GitHub Actions workspace-rust-ci.yml pipeline locally with
// file-hash caching and sccache integration for fast, accurate pre-push
// validation.
//
// Usage:
//
//	lci                     Run default stages (fmt, clippy, test)
//	lci fmt clippy          Run specific stages
//	lci --no-cache          Disable hash cache, force all stages
//	lci --fix               Auto-fix formatting (cargo fmt without --check)
//	lci --verbose           Show cache decisions and full output
//	lci --init              Generate .lci.toml with workspace defaults
//	lci --env-check         Verify system dependencies (protoc, llvm, etc.)
package main

import (
	"flag"
	"fmt"
	"os"
	"time"
)

var version = "0.1.0"

func main() {
	var (
		flagNoCache  = flag.Bool("no-cache", false, "Disable source-hash cache, run all stages unconditionally")
		flagVerbose  = flag.Bool("verbose", false, "Show cache decisions and detailed stage output")
		flagCI       = flag.Bool("ci", false, "CI mode: disable colors, structured output")
		flagFix      = flag.Bool("fix", false, "Auto-fix formatting (runs cargo fmt without --check)")
		flagFailFast = flag.Bool("fail-fast", true, "Stop pipeline on first stage failure")
		flagJobs     = flag.Int("jobs", 0, "Parallel cargo jobs (0 = auto-detect CPU count)")
		flagInit     = flag.Bool("init", false, "Create default .lci.toml in the workspace root")
		flagEnvCheck = flag.Bool("env-check", false, "Check system dependencies match CI requirements")
		flagList     = flag.Bool("list", false, "List available stages and exit")
		flagVersion  = flag.Bool("version", false, "Print version and exit")
	)

	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "lci v%s — Local CI for Rust workspaces\n\n", version)
		fmt.Fprintf(os.Stderr, "Usage: lci [flags] [stages...]\n\n")
		fmt.Fprintf(os.Stderr, "Stages:\n")
		fmt.Fprintf(os.Stderr, "  fmt      Check formatting (cargo fmt --check)\n")
		fmt.Fprintf(os.Stderr, "  clippy   Lint workspace (cargo clippy -D warnings)\n")
		fmt.Fprintf(os.Stderr, "  test     Run workspace tests (with configured exclusions)\n")
		fmt.Fprintf(os.Stderr, "  check    Quick compile check (cargo check)\n")
		fmt.Fprintf(os.Stderr, "  build    Full release build (cargo build)\n\n")
		fmt.Fprintf(os.Stderr, "Flags:\n")
		flag.PrintDefaults()
	}
	flag.Parse()

	if *flagVersion {
		fmt.Printf("lci v%s\n", version)
		return
	}

	ui := newUI(!*flagCI)

	if *flagList {
		ui.listStages()
		return
	}

	// Find the Cargo workspace root
	cwd, err := os.Getwd()
	if err != nil {
		ui.fatal("Cannot determine working directory: %v", err)
	}

	root, err := findWorkspaceRoot(cwd)
	if err != nil {
		ui.fatal("%v", err)
	}

	// --init: generate config and exit
	if *flagInit {
		if err := generateDefaultConfig(root); err != nil {
			ui.fatal("Failed to create .lci.toml: %v", err)
		}
		ui.success("Created .lci.toml in %s", root)
		return
	}

	// Load workspace config
	cfg, err := loadConfig(root)
	if err != nil {
		ui.fatal("Config error: %v", err)
	}

	if *flagJobs > 0 {
		cfg.Jobs = *flagJobs
	}

	// --env-check: verify dependencies and exit
	if *flagEnvCheck {
		checkEnv(ui, cfg)
		return
	}

	// Determine stages
	stages := flag.Args()
	if len(stages) == 0 {
		stages = cfg.DefaultStages
	}

	ui.header(root, len(cfg.Members), len(cfg.TestExclude))

	// Compute source hash for cache
	start := time.Now()
	sourceHash, err := computeSourceHash(root)
	if err != nil {
		if *flagVerbose {
			ui.warn("Hash computation failed: %v (cache disabled)", err)
		}
		*flagNoCache = true
	}

	// Load cache
	var cache *Cache
	if !*flagNoCache {
		cache, err = loadCache(root)
		if err != nil && *flagVerbose {
			ui.warn("Cache load failed: %v", err)
		}
	}

	// Detect sccache
	if cfg.UseSccache {
		if path := detectSccache(); path != "" {
			cfg.SccachePath = path
			if *flagVerbose {
				ui.info("sccache detected: %s", path)
			}
		}
	}

	// Build and run the pipeline
	pipeline := newPipeline(cfg, stages, *flagFix)
	results := pipeline.run(root, cache, sourceHash, ui, *flagVerbose, *flagFailFast)

	// Persist cache for successful stages
	if cache != nil {
		for _, r := range results {
			if r.Status == statusPass {
				cache.set(r.Name, sourceHash)
			}
		}
		if err := saveCache(cache, root); err != nil && *flagVerbose {
			ui.warn("Cache save failed: %v", err)
		}
	}

	elapsed := time.Since(start)
	ui.summary(results, elapsed)

	for _, r := range results {
		if r.Status == statusFail {
			os.Exit(1)
		}
	}
}
