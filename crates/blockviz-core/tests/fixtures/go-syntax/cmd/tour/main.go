// Command tour draws shapes and keeps them in a store.
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"

	"example.com/tour/shapes"
	"example.com/tour/store"
)

var verbose = flag.Bool("v", false, "print every shape")

func main() {
	flag.Parse()
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	db, err := store.Open(ctx, os.Getenv("DATABASE_URL"))
	if err != nil {
		log.Fatalf("open store: %v", err)
	}
	defer db.Close()

	for _, s := range shapes.Defaults() {
		if err := db.Save(ctx, s); err != nil {
			log.Printf("save %s: %v", s, err)
			continue
		}
		if *verbose {
			fmt.Println(s, shapes.Total(s))
		}
	}
}
