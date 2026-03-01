package cmd

import (
	"fmt"
	"net/url"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var sourceCmd = &cobra.Command{
	Use:   "source",
	Short: "Manage knowledge sources",
}

var sourceIngestCmd = &cobra.Command{
	Use:   "ingest",
	Short: "Ingest a new source",
	Run: func(cmd *cobra.Command, args []string) {
		content, _ := cmd.Flags().GetString("content")
		stype, _ := cmd.Flags().GetString("type")
		title, _ := cmd.Flags().GetString("title")
		sourceURL, _ := cmd.Flags().GetString("url")
		if content == "" {
			internal.Die("--content is required")
		}
		body := map[string]interface{}{
			"content":     content,
			"source_type": stype,
		}
		if title != "" {
			body["title"] = title
		}
		if sourceURL != "" {
			body["url"] = sourceURL
		}
		resp, err := client.Post("/sources", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Source ingested:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sourceGetCmd = &cobra.Command{
	Use:   "get <id>",
	Short: "Get a source by ID",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/sources/"+args[0], nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sourceListCmd = &cobra.Command{
	Use:   "list",
	Short: "List sources",
	Run: func(cmd *cobra.Command, args []string) {
		stype, _ := cmd.Flags().GetString("type")
		status, _ := cmd.Flags().GetString("status")
		p := url.Values{}
		if stype != "" {
			p.Set("source_type", stype)
		}
		if status != "" {
			p.Set("status", status)
		}
		resp, err := client.Get("/sources", p)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sourceDeleteCmd = &cobra.Command{
	Use:   "delete <id>",
	Short: "Delete a source",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		_, err := client.Delete("/sources/" + args[0])
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Printf("Source %s deleted.\n", args[0])
	},
}

func init() {
	sourceCmd.AddCommand(sourceIngestCmd, sourceGetCmd, sourceListCmd, sourceDeleteCmd)

	sourceIngestCmd.Flags().String("content", "", "Source content (required)")
	sourceIngestCmd.Flags().String("type", "document", "Source type (document, web, conversation, observation, tool_output, user_input)")
	sourceIngestCmd.Flags().String("title", "", "Optional title")
	sourceIngestCmd.Flags().String("url", "", "Optional canonical URL")

	sourceListCmd.Flags().String("type", "", "Filter by source type")
	sourceListCmd.Flags().String("status", "", "Filter by status")

	rootCmd.AddCommand(sourceCmd)
}
