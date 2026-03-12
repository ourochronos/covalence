package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	memoryTopic string
	memoryLimit int
)

var memoryCmd = &cobra.Command{
	Use:   "memory",
	Short: "Memory operations (store, recall, forget)",
	Long:  "Store, recall, and manage memories in the knowledge engine.",
}

var memoryStoreCmd = &cobra.Command{
	Use:   "store [content]",
	Short: "Store a memory",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		body := map[string]interface{}{
			"content": args[0],
		}
		if memoryTopic != "" {
			body["topic"] = memoryTopic
		}

		var result map[string]interface{}
		if err := client.Post("/memory", body, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Memory stored: %s\n", shortID(getString(result, "id")))
		return nil
	},
}

var memoryRecallCmd = &cobra.Command{
	Use:   "recall [query]",
	Short: "Recall memories matching a query",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		body := map[string]interface{}{
			"query": args[0],
			"limit": memoryLimit,
		}
		if memoryTopic != "" {
			body["topic"] = memoryTopic
		}

		var result []map[string]interface{}
		if err := client.Post("/memory/recall", body, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Relevance", "Content", "Topic"}
		rows := make([][]string, 0, len(result))
		for _, m := range result {
			content := truncateRunes(getString(m, "content"), 60)
			rows = append(rows, []string{
				shortID(getString(m, "id")),
				fmt.Sprintf("%.4f", getFloat(m, "relevance")),
				content,
				getString(m, "topic"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var memoryForgetCmd = &cobra.Command{
	Use:   "forget [id]",
	Short: "Forget (delete) a memory",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Delete("/memory/"+args[0], &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Memory %s forgotten\n", shortID(args[0]))
		return nil
	},
}

var memoryStatusCmd = &cobra.Command{
	Use:   "status",
	Short: "Get memory subsystem status",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Get("/memory/status", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Total Memories:      %s\n", getString(result, "total_memories"))
		fmt.Printf("Total Entities:      %s\n", getString(result, "total_entities"))
		fmt.Printf("Total Relationships: %s\n", getString(result, "total_relationships"))
		fmt.Printf("Communities:         %s\n", getString(result, "communities"))
		return nil
	},
}

func init() {
	memoryStoreCmd.Flags().StringVar(&memoryTopic, "topic", "",
		"Topic/category for the memory")
	memoryRecallCmd.Flags().IntVar(&memoryLimit, "limit", 10,
		"Maximum memories to return")
	memoryRecallCmd.Flags().StringVar(&memoryTopic, "topic", "",
		"Filter by topic")
	memoryCmd.AddCommand(memoryStoreCmd)
	memoryCmd.AddCommand(memoryRecallCmd)
	memoryCmd.AddCommand(memoryForgetCmd)
	memoryCmd.AddCommand(memoryStatusCmd)
	rootCmd.AddCommand(memoryCmd)
}
