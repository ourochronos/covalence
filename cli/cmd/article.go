package cmd

import (
	"fmt"
	"strings"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var articleCmd = &cobra.Command{
	Use:   "article",
	Short: "Manage knowledge articles",
}

var articleCreateCmd = &cobra.Command{
	Use:   "create",
	Short: "Create a new article",
	Run: func(cmd *cobra.Command, args []string) {
		content, _ := cmd.Flags().GetString("content")
		title, _ := cmd.Flags().GetString("title")
		sourceIDs, _ := cmd.Flags().GetString("source-ids")
		if content == "" {
			internal.Die("--content is required")
		}
		body := map[string]interface{}{"content": content}
		if title != "" {
			body["title"] = title
		}
		if sourceIDs != "" {
			body["source_ids"] = strings.Split(sourceIDs, ",")
		}
		resp, err := client.Post("/articles", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Article created:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleGetCmd = &cobra.Command{
	Use:   "get <id>",
	Short: "Get an article by ID",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/articles/"+args[0], nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleUpdateCmd = &cobra.Command{
	Use:   "update <id>",
	Short: "Update an article's content",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		content, _ := cmd.Flags().GetString("content")
		sourceID, _ := cmd.Flags().GetString("source-id")
		if content == "" {
			internal.Die("--content is required")
		}
		body := map[string]interface{}{"content": content}
		if sourceID != "" {
			body["source_id"] = sourceID
		}
		resp, err := client.Patch("/articles/"+args[0], body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Article updated:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleDeleteCmd = &cobra.Command{
	Use:   "delete <id>",
	Short: "Delete an article",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		_, err := client.Delete("/articles/" + args[0])
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Printf("Article %s deleted.\n", args[0])
	},
}

var articleSplitCmd = &cobra.Command{
	Use:   "split <id>",
	Short: "Split an article into two",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Post("/articles/"+args[0]+"/split", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Article split:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleMergeCmd = &cobra.Command{
	Use:   "merge <id1> <id2>",
	Short: "Merge two articles",
	Args:  cobra.ExactArgs(2),
	Run: func(cmd *cobra.Command, args []string) {
		body := map[string]interface{}{
			"article_id_a": args[0],
			"article_id_b": args[1],
		}
		resp, err := client.Post("/articles/merge", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Articles merged:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleCompileCmd = &cobra.Command{
	Use:   "compile",
	Short: "Compile sources into an article",
	Run: func(cmd *cobra.Command, args []string) {
		sourceIDs, _ := cmd.Flags().GetString("source-ids")
		titleHint, _ := cmd.Flags().GetString("title-hint")
		if sourceIDs == "" {
			internal.Die("--source-ids is required")
		}
		body := map[string]interface{}{
			"source_ids": strings.Split(sourceIDs, ","),
		}
		if titleHint != "" {
			body["title_hint"] = titleHint
		}
		resp, err := client.Post("/articles/compile", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Article compiled:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var articleProvenanceCmd = &cobra.Command{
	Use:   "provenance <id>",
	Short: "Get provenance for an article",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/articles/"+args[0]+"/provenance", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

func init() {
	articleCmd.AddCommand(
		articleCreateCmd, articleGetCmd, articleUpdateCmd, articleDeleteCmd,
		articleSplitCmd, articleMergeCmd, articleCompileCmd, articleProvenanceCmd,
	)

	articleCreateCmd.Flags().String("content", "", "Article content (required)")
	articleCreateCmd.Flags().String("title", "", "Optional title")
	articleCreateCmd.Flags().String("source-ids", "", "Comma-separated source IDs")

	articleUpdateCmd.Flags().String("content", "", "New content (required)")
	articleUpdateCmd.Flags().String("source-id", "", "Source ID triggering update")

	articleCompileCmd.Flags().String("source-ids", "", "Comma-separated source IDs (required)")
	articleCompileCmd.Flags().String("title-hint", "", "Optional title hint")

	rootCmd.AddCommand(articleCmd)
}
