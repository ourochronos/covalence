package cmd

import (
	"fmt"
	"strings"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	askMaxContext int
	askStrategy   string
)

var askCmd = &cobra.Command{
	Use:   "ask [question]",
	Short: "Ask the knowledge graph a question",
	Long: `Synthesize an answer from the knowledge graph using LLM reasoning.

Searches across all dimensions to gather relevant context, enriches it
with provenance and confidence metadata, and sends it to an LLM for
grounded synthesis. Returns a structured answer with citations.`,
	Args: cobra.MinimumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		question := strings.Join(args, " ")

		body := map[string]interface{}{
			"question":    question,
			"max_context": askMaxContext,
		}
		if askStrategy != "auto" {
			body["strategy"] = askStrategy
		}

		var result map[string]interface{}
		if err := client.Post("/ask", body, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		answer := getString(result, "answer")
		contextUsed := int(getFloat(result, "context_used"))

		fmt.Printf("Context: %d fragments used\n\n", contextUsed)
		fmt.Println(answer)

		// Print citations
		citations, ok := result["citations"].([]interface{})
		if ok && len(citations) > 0 {
			fmt.Println("\n--- Citations ---")
			for i, raw := range citations {
				cit, ok := raw.(map[string]interface{})
				if !ok {
					continue
				}
				source := getString(cit, "source")
				resultType := getString(cit, "result_type")
				confidence := getFloat(cit, "confidence")
				snippet := getString(cit, "snippet")

				fmt.Printf("\n[%d] %s (%s, confidence: %.2f)\n",
					i+1, source, resultType, confidence)
				if snippet != "" {
					fmt.Println(truncateRunes(snippet, 200))
				}
			}
		}

		return nil
	},
}

func init() {
	askCmd.Flags().IntVar(&askMaxContext, "max-context", 15,
		"Maximum search results to include as context")
	askCmd.Flags().StringVar(&askStrategy, "strategy", "auto",
		"Search strategy (auto, balanced, precise, exploratory, recent, graph_first, global)")
	rootCmd.AddCommand(askCmd)
}
