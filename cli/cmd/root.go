// Package cmd implements the Cobra CLI commands for Covalence.
package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"
)

var (
	apiURL     string
	jsonOutput bool
)

var rootCmd = &cobra.Command{
	Use:   "cove",
	Short: "Covalence knowledge engine CLI",
	Long:  "cove is a CLI for interacting with the Covalence knowledge engine.",
}

// Execute runs the root command.
func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func init() {
	rootCmd.PersistentFlags().StringVar(&apiURL, "api-url", "http://localhost:8431", "Covalence API URL")
	rootCmd.PersistentFlags().BoolVar(&jsonOutput, "json", false, "Output in JSON format")
}
