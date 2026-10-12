package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

var verbose bool

var rootCmd = &cobra.Command{
	Use:   "shop",
	Short: "Shop runs the store",
	RunE: func(cmd *cobra.Command, args []string) error {
		fmt.Println("running")
		return nil
	},
}

var serveCmd = &cobra.Command{
	Use:   "serve",
	Short: "Serve the API",
	RunE:  serve,
}

func serve(cmd *cobra.Command, args []string) error {
	return nil
}

func init() {
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")
	rootCmd.AddCommand(serveCmd)
}

// Execute runs the command line.
func Execute() error {
	return rootCmd.Execute()
}
