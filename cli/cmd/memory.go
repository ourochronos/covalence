package cmd

import (
	"fmt"
	"strings"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var memoryCmd = &cobra.Command{
	Use:   "memory",
	Short: "Store and recall memories",
}

var memoryStoreCmd = &cobra.Command{
	Use:   "store",
	Short: "Store a new memory",
	Run: func(cmd *cobra.Command, args []string) {
		content, _ := cmd.Flags().GetString("content")
		tagsStr, _ := cmd.Flags().GetString("tags")
		importance, _ := cmd.Flags().GetFloat64("importance")
		context, _ := cmd.Flags().GetString("context")
		supersedesID, _ := cmd.Flags().GetString("supersedes")

		if content == "" {
			internal.Die("--content is required")
		}

		body := map[string]interface{}{
			"content":    content,
			"importance": importance,
		}
		if tagsStr != "" {
			body["tags"] = strings.Split(tagsStr, ",")
		}
		if context != "" {
			body["context"] = context
		}
		if supersedesID != "" {
			body["supersedes_id"] = supersedesID
		}

		resp, err := client.Post("/memory", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Memory stored:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var memoryRecallCmd = &cobra.Command{
	Use:   "recall",
	Short: "Recall memories by query",
	Run: func(cmd *cobra.Command, args []string) {
		query, _ := cmd.Flags().GetString("query")
		limit, _ := cmd.Flags().GetInt("limit")
		tagsStr, _ := cmd.Flags().GetString("tags")
		minConf, _ := cmd.Flags().GetFloat64("min-confidence")

		if query == "" {
			internal.Die("--query is required")
		}

		body := map[string]interface{}{
			"query": query,
			"limit": limit,
		}
		if tagsStr != "" {
			body["tags"] = strings.Split(tagsStr, ",")
		}
		if cmd.Flags().Changed("min-confidence") {
			body["min_confidence"] = minConf
		}

		// Engine route: POST /memory/search
		resp, err := client.Post("/memory/search", body)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var memoryForgetCmd = &cobra.Command{
	Use:   "forget <id>",
	Short: "Forget (soft-delete) a memory",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		reason, _ := cmd.Flags().GetString("reason")
		body := map[string]interface{}{}
		if reason != "" {
			body["reason"] = reason
		}
		// Engine route: PATCH /memory/{id}/forget
		resp, err := client.Patch("/memory/"+args[0]+"/forget", body)
		if err != nil {
			internal.Die("%v", err)
		}
		if resp.StatusCode == 204 || len(resp.Body) < 3 {
			fmt.Printf("Memory %s forgotten.\n", args[0])
		} else {
			fmt.Printf("Memory %s forgotten.\n", args[0])
			internal.ParseAndPrint(resp.Body, jsonMode)
		}
	},
}

var memoryStatusCmd = &cobra.Command{
	Use:   "status",
	Short: "Get memory system statistics",
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/memory/status", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

func init() {
	memoryCmd.AddCommand(memoryStoreCmd, memoryRecallCmd, memoryForgetCmd, memoryStatusCmd)

	memoryStoreCmd.Flags().String("content", "", "Memory content (required)")
	memoryStoreCmd.Flags().String("tags", "", "Comma-separated tags")
	memoryStoreCmd.Flags().Float64("importance", 0.5, "Importance score (0.0-1.0)")
	memoryStoreCmd.Flags().String("context", "", "Context string")
	memoryStoreCmd.Flags().String("supersedes", "", "UUID of memory this replaces")

	memoryRecallCmd.Flags().String("query", "", "Recall query (required)")
	memoryRecallCmd.Flags().Int("limit", 5, "Maximum results")
	memoryRecallCmd.Flags().String("tags", "", "Comma-separated tag filter")
	memoryRecallCmd.Flags().Float64("min-confidence", 0, "Minimum confidence threshold")

	memoryForgetCmd.Flags().String("reason", "", "Reason for forgetting")

	rootCmd.AddCommand(memoryCmd)
}
