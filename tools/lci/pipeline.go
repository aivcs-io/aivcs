package main

import (
	"fmt"
	"time"
)

// stageStatus represents the outcome of a pipeline stage.
type stageStatus int

const (
	statusPending stageStatus = iota
	statusRunning
	statusPass
	statusFail
	statusSkip
)

func (s stageStatus) String() string {
	switch s {
	case statusPass:
		return "PASS"
	case statusFail:
		return "FAIL"
	case statusSkip:
		return "SKIP"
	case statusRunning:
		return "RUN"
	default:
		return "PEND"
	}
}

// stageResult captures the outcome of a single pipeline stage.
type stageResult struct {
	Name     string
	Status   stageStatus
	Duration time.Duration
	Output   string
	Command  string
}

// stageSpec defines a pipeline stage — the command to run and its arguments.
type stageSpec struct {
	Name string
	Cmd  string
	Args []string
}

// pipeline orchestrates the sequential execution of CI stages.
type pipeline struct {
	stages []stageSpec
	config *Config
}

// newPipeline constructs a pipeline from the requested stage names.
func newPipeline(cfg *Config, stageNames []string, fix bool) *pipeline {
	var specs []stageSpec

	for _, name := range stageNames {
		switch name {
		case "fmt":
			if fix {
				specs = append(specs, stageSpec{
					Name: "fmt",
					Cmd:  "cargo",
					Args: []string{"fmt", "--all"},
				})
			} else {
				specs = append(specs, stageSpec{
					Name: "fmt",
					Cmd:  "cargo",
					Args: []string{"fmt", "--all", "--", "--check"},
				})
			}

		case "clippy":
			specs = append(specs, stageSpec{
				Name: "clippy",
				Cmd:  "cargo",
				Args: []string{"clippy", "--workspace", "--all-targets", "--", "-D", "warnings"},
			})

		case "test":
			args := []string{"test", "--workspace"}
			args = append(args, buildExcludeArgs(cfg.TestExclude)...)
			specs = append(specs, stageSpec{
				Name: "test",
				Cmd:  "cargo",
				Args: args,
			})

		case "check":
			specs = append(specs, stageSpec{
				Name: "check",
				Cmd:  "cargo",
				Args: []string{"check", "--workspace"},
			})

		case "build":
			specs = append(specs, stageSpec{
				Name: "build",
				Cmd:  "cargo",
				Args: []string{"build", "--workspace"},
			})

		default:
			// Unknown stages are treated as cargo subcommands for extensibility
			specs = append(specs, stageSpec{
				Name: name,
				Cmd:  "cargo",
				Args: []string{name, "--workspace"},
			})
		}
	}

	return &pipeline{stages: specs, config: cfg}
}

// run executes all stages sequentially, respecting cache and fail-fast.
func (p *pipeline) run(root string, cache *Cache, sourceHash string, ui *UI, verbose, failFast bool) []stageResult {
	env := cargoEnv(p.config)
	var results []stageResult

	for _, spec := range p.stages {
		// Check cache
		if cache != nil && sourceHash != "" {
			if cached, ok := cache.get(spec.Name); ok && cached == sourceHash {
				ui.stageSkip(spec.Name)
				results = append(results, stageResult{
					Name:    spec.Name,
					Status:  statusSkip,
					Command: formatCommand(spec.Cmd, spec.Args),
				})
				continue
			}
		}

		// Apply jobs flag if configured
		args := spec.Args
		if p.config.Jobs > 0 {
			args = append(args, fmt.Sprintf("-j%d", p.config.Jobs))
		}

		cmdStr := formatCommand(spec.Cmd, args)
		ui.stageStart(spec.Name, cmdStr)

		result := runCommand(root, env, verbose, spec.Cmd, args...)

		sr := stageResult{
			Name:     spec.Name,
			Duration: result.Duration,
			Output:   result.Output,
			Command:  cmdStr,
		}

		if result.ExitCode == 0 {
			sr.Status = statusPass
			ui.stagePass(spec.Name, result.Duration)
		} else {
			sr.Status = statusFail
			ui.stageFail(spec.Name, result.Duration)
			// Show captured output on failure when not in verbose mode
			if !verbose && result.Output != "" {
				ui.stageOutput(result.Output)
			}
		}

		results = append(results, sr)

		if failFast && sr.Status == statusFail {
			// Mark remaining stages as skipped
			for _, remaining := range p.stages[len(results):] {
				results = append(results, stageResult{
					Name:    remaining.Name,
					Status:  statusSkip,
					Command: formatCommand(remaining.Cmd, remaining.Args),
				})
			}
			break
		}
	}

	return results
}
