package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/spf13/cobra"
)

var llmModel string

var llmCmd = &cobra.Command{
	Use:     "llm [prompt]",
	Aliases: []string{"copilot"},
	Short:   "Run a prompt through an LLM backend",
	Long: `Run a prompt through a configurable LLM backend.

Providers:
  claude   Claude via the claude CLI (haiku, sonnet, opus)
  gemini   Gemini via the gemini CLI
  copilot  GitHub Copilot via the copilot CLI

Models:
  haiku    Claude Haiku 4.5 via claude CLI
  sonnet   Claude Sonnet 4.5 via claude CLI
  opus     Claude Opus 4.6 via claude CLI
  gemini   Gemini 2.5 Flash via gemini CLI
  copilot  Claude Haiku 4.5 via copilot CLI

The model can also be set via the COVALENCE_LLM_MODEL environment variable
(falls back to COVALENCE_COPILOT_MODEL for backwards compatibility).

Examples:
  cove llm "Explain how entity resolution works"
  cove llm --model haiku "Summarize this module"
  cove llm --model gemini "Review the diffs on this branch"
  cove llm --model copilot "Review the unpushed commits"
  cove copilot "This still works as an alias"`,
	Args: cobra.MinimumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		prompt := strings.Join(args, " ")
		model := llmModel
		if model == "" {
			model = os.Getenv("COVALENCE_LLM_MODEL")
		}
		if model == "" {
			model = os.Getenv("COVALENCE_COPILOT_MODEL")
		}
		if model == "" {
			model = "haiku"
		}

		switch model {
		case "haiku":
			return runClaudeCLI(prompt, "haiku")
		case "sonnet":
			return runClaudeCLI(prompt, "sonnet")
		case "opus":
			return runClaudeCLI(prompt, "opus")
		case "claude":
			return runClaudeCLI(prompt, "haiku")
		case "gemini":
			return runGeminiCLI(prompt)
		case "copilot":
			return runCopilotCLI(prompt)
		default:
			return fmt.Errorf(
				"unknown model %q — use haiku, sonnet, opus, gemini, or copilot",
				model,
			)
		}
	},
}

func runGeminiCLI(prompt string) error {
	bin, err := exec.LookPath("gemini")
	if err != nil {
		return fmt.Errorf("gemini CLI not found: %w", err)
	}
	c := exec.Command(bin, "-p", prompt)
	c.Stdout = os.Stdout
	c.Stderr = os.Stderr
	return c.Run()
}

func runClaudeCLI(prompt string, model string) error {
	bin, err := exec.LookPath("claude")
	if err != nil {
		return fmt.Errorf("claude CLI not found: %w", err)
	}
	c := exec.Command(bin, "--print", "--model", model, prompt)
	c.Stdout = os.Stdout
	c.Stderr = os.Stderr
	return c.Run()
}

func runCopilotCLI(prompt string) error {
	bin, err := exec.LookPath("copilot")
	if err != nil {
		return fmt.Errorf("copilot CLI not found: %w", err)
	}
	c := exec.Command(bin, "-p", prompt, "--model", "claude-haiku-4.5")
	c.Stdout = os.Stdout
	c.Stderr = os.Stderr
	return c.Run()
}

func init() {
	llmCmd.Flags().StringVar(&llmModel, "model", "",
		"LLM backend: haiku (default), sonnet, opus, gemini, copilot")
	rootCmd.AddCommand(llmCmd)
}
