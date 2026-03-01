package cmd

import (
	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var searchCmd = &cobra.Command{
	Use:   "search",
	Short: "Hybrid semantic search over articles and sources",
	Run: func(cmd *cobra.Command, args []string) {
		query, _ := cmd.Flags().GetString("query")
		intent, _ := cmd.Flags().GetString("intent")
		limit, _ := cmd.Flags().GetInt("limit")
		explain, _ := cmd.Flags().GetBool("explain")
		includeSources, _ := cmd.Flags().GetBool("include-sources")

		if query == "" {
			internal.Die("--query is required")
		}

		body := map[string]interface{}{
			"query": query,
			"limit": limit,
		}
		if intent != "" {
			body["intent"] = intent
		}
		if explain {
			body["explain"] = true
		}
		if includeSources {
			body["include_sources"] = true
		}

		resp, err := client.Post("/search", body)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

func init() {
	searchCmd.Flags().String("query", "", "Search query (required)")
	searchCmd.Flags().String("intent", "", "Query intent (factual, temporal, causal, entity)")
	searchCmd.Flags().Int("limit", 10, "Maximum results to return")
	searchCmd.Flags().Bool("explain", false, "Include scoring explanation")
	searchCmd.Flags().Bool("include-sources", false, "Include raw sources in results")

	rootCmd.AddCommand(searchCmd)
}
