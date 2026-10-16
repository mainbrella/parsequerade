package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/signal"

	parsequerade "github.com/mainbrella/parsequerade/sdks/go"
)

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}

func run() error {
	privateKey, err := os.ReadFile(os.Getenv("ASC_PRIVATE_KEY_PATH"))
	if err != nil {
		return fmt.Errorf("read ASC_PRIVATE_KEY_PATH: %w", err)
	}
	client, err := parsequerade.NewClient(parsequerade.Credentials{
		IssuerID:   os.Getenv("ASC_ISSUER_ID"),
		KeyID:      os.Getenv("ASC_KEY_ID"),
		PrivateKey: privateKey,
	}, nil)
	if err != nil {
		return err
	}
	defer client.Close()
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()
	query := &parsequerade.AppsQuery{Limit: 50}
	for {
		page, err := client.Apps(ctx, query)
		if err != nil {
			return err
		}
		for _, app := range page.Items {
			fmt.Printf("%s\t%s\n", app.ID, app.Name.Value)
		}
		if page.NextCursor == nil {
			return nil
		}
		query.Cursor = *page.NextCursor
	}
}
