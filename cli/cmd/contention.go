package cmd

import (
	"fmt"
	"net/url"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var contentionCmd = &cobra.Command{
	Use:   "contention",
	Short: "Manage knowledge contentions",
}

var contentionListCmd = &cobra.Command{
	Use:   "list",
	Short: "List contentions",
	Run: func(cmd *cobra.Command, args []string) {
		status, _ := cmd.Flags().GetString("status")
		articleID, _ := cmd.Flags().GetString("article-id")

		p := url.Values{}
		if status != "" {
			p.Set("status", status)
		}
		if articleID != "" {
			p.Set("article_id", articleID)
		}

		resp, err := client.Get("/contentions", p)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var contentionResolveCmd = &cobra.Command{
	Use:   "resolve <id>",
	Short: "Resolve a contention",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resolution, _ := cmd.Flags().GetString("resolution")
		rationale, _ := cmd.Flags().GetString("rationale")

		if resolution == "" {
			internal.Die("--resolution is required (supersede_a, supersede_b, accept_both, dismiss)")
		}

		body := map[string]interface{}{
			"resolution": resolution,
		}
		if rationale != "" {
			body["rationale"] = rationale
		}

		resp, err := client.Post("/contentions/"+args[0]+"/resolve", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Contention resolved:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

func init() {
	contentionCmd.AddCommand(contentionListCmd, contentionResolveCmd)

	contentionListCmd.Flags().String("status", "", "Filter by status (detected, resolved, dismissed)")
	contentionListCmd.Flags().String("article-id", "", "Filter by article ID")

	contentionResolveCmd.Flags().String("resolution", "", "Resolution type: supersede_a, supersede_b, accept_both, dismiss (required)")
	contentionResolveCmd.Flags().String("rationale", "", "Rationale for the resolution")

	rootCmd.AddCommand(contentionCmd)
}
