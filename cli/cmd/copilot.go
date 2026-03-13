package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/spf13/cobra"
)

var copilotModel string

var copilotCmd = &cobra.Command{
	Use:   "copilot [prompt]",
	Short: "Run a prompt through an LLM backend",
	Long: `Run a prompt through a configurable LLM backend.

Models:
  gemini   Gemini Pro 3.0 via the gemini CLI (default for code review)
  haiku    Claude Haiku 4.5 via the claude CLI (default for extraction)

The model can also be set via the COVALENCE_COPILOT_MODEL environment variable.

Examples:
  cove copilot "Review the unpushed commits for code quality"
  cove copilot --model gemini "Review the diffs on this branch"
  cove copilot --model haiku "Summarize this code module"`,
	Args: cobra.MinimumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		prompt := strings.Join(args, " ")
		model := copilotModel
		if model == "" {
			model = os.Getenv("COVALENCE_COPILOT_MODEL")
		}
		if model == "" {
			model = "gemini"
		}

		switch model {
		case "gemini":
			return runGemini(prompt)
		case "haiku":
			return runClaude(prompt, "haiku")
		case "sonnet":
			return runClaude(prompt, "sonnet")
		case "opus":
			return runClaude(prompt, "opus")
		default:
			return fmt.Errorf(
				"unknown model %q — use gemini, haiku, sonnet, or opus",
				model,
			)
		}
	},
}

func runGemini(prompt string) error {
	bin, err := exec.LookPath("gemini")
	if err != nil {
		return fmt.Errorf("gemini CLI not found: %w", err)
	}
	c := exec.Command(bin, "-p", prompt)
	c.Stdout = os.Stdout
	c.Stderr = os.Stderr
	return c.Run()
}

func runClaude(prompt string, model string) error {
	bin, err := exec.LookPath("claude")
	if err != nil {
		return fmt.Errorf("claude CLI not found: %w", err)
	}
	c := exec.Command(bin, "--print", "--model", model, prompt)
	c.Stdout = os.Stdout
	c.Stderr = os.Stderr
	return c.Run()
}

func init() {
	copilotCmd.Flags().StringVar(&copilotModel, "model", "",
		"LLM backend: gemini (default), haiku, sonnet, opus")
	rootCmd.AddCommand(copilotCmd)
}
