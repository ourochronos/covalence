package cmd

import (
	"net/url"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var adminCmd = &cobra.Command{
	Use:   "admin",
	Short: "Administrative operations",
}

var adminStatsCmd = &cobra.Command{
	Use:   "stats",
	Short: "Get system statistics",
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/admin/stats", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var adminMaintenanceCmd = &cobra.Command{
	Use:   "maintenance",
	Short: "Trigger maintenance operations",
	Run: func(cmd *cobra.Command, args []string) {
		recompute, _ := cmd.Flags().GetBool("recompute-scores")
		processQ, _ := cmd.Flags().GetBool("process-queue")
		evict, _ := cmd.Flags().GetBool("evict")
		evictCount, _ := cmd.Flags().GetInt("evict-count")

		body := map[string]interface{}{}
		if recompute {
			body["recompute_scores"] = true
		}
		if processQ {
			body["process_queue"] = true
		}
		if evict {
			body["evict_if_over_capacity"] = true
			if evictCount > 0 {
				body["evict_count"] = evictCount
			}
		}

		resp, err := client.Post("/admin/maintenance", body)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var adminQueueCmd = &cobra.Command{
	Use:   "queue",
	Short: "View the mutation queue",
	Run: func(cmd *cobra.Command, args []string) {
		status, _ := cmd.Flags().GetString("status")
		p := url.Values{}
		if status != "" {
			p.Set("status", status)
		}
		resp, err := client.Get("/admin/queue", p)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

func init() {
	adminCmd.AddCommand(adminStatsCmd, adminMaintenanceCmd, adminQueueCmd)

	adminMaintenanceCmd.Flags().Bool("recompute-scores", false, "Recompute usage scores for all articles")
	adminMaintenanceCmd.Flags().Bool("process-queue", false, "Process pending mutation queue entries")
	adminMaintenanceCmd.Flags().Bool("evict", false, "Evict articles if over capacity")
	adminMaintenanceCmd.Flags().Int("evict-count", 10, "Max articles to evict per run")

	adminQueueCmd.Flags().String("status", "", "Filter by queue status (e.g. pending)")

	rootCmd.AddCommand(adminCmd)
}
