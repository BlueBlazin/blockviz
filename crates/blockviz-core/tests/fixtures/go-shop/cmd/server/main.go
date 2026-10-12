package main

import (
	"log"
	"net/http"
	"os"

	"github.com/jmoiron/sqlx"

	"example.com/shop/api"
)

func health(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
}

func item(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	w.Write([]byte(id))
}

func main() {
	db, err := sqlx.Connect("postgres", os.Getenv("DATABASE_URL"))
	if err != nil {
		log.Fatal(err)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/healthz", health)
	mux.HandleFunc("GET /items/{id}", item)
	go http.ListenAndServe(":8081", mux)
	if err := api.New(db).Run(":8080"); err != nil {
		log.Fatal(err)
	}
}
