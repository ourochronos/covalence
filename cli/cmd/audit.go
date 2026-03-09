package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var auditLimit int

var auditCmd = &cobra.Command{
	Use:   "audit",
	Short: "View audit log",
	Long:  "List audit log entries.",
}

var auditListCmd = &cobra.Command{
	Use:   "list",
	Short: "List audit log entries",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		path := fmt.Sprintf("/audit?limit=%d", auditLimit)

		var result []map[string]interface{}
		if err := client.Get(path, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Action", "Actor", "Target Type", "Target ID", "Created At"}
		rows := make([][]string, 0, len(result))
		for _, e := range result {
			rows = append(rows, []string{
				shortID(getString(e, "id")),
				getString(e, "action"),
				getString(e, "actor"),
				getString(e, "target_type"),
				shortID(getString(e, "target_id")),
				getString(e, "created_at"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

func init() {
	auditListCmd.Flags().IntVar(&auditLimit, "limit", 20,
		"Maximum entries to return")
	auditCmd.AddCommand(auditListCmd)
	rootCmd.AddCommand(auditCmd)
}
