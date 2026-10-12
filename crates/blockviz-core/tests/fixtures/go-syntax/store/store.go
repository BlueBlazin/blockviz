// Package store keeps shapes in a Postgres database.
package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"sync"

	_ "github.com/lib/pq"

	"example.com/tour/shapes"
)

// ErrClosed is returned after Close.
var ErrClosed = errors.New("store: closed")

// NotFoundError says which key was missing.
type NotFoundError struct {
	Key string
}

func (e *NotFoundError) Error() string { return "not found: " + e.Key }

// DB is a store of shapes.
type DB struct {
	mu     sync.Mutex
	conn   *sql.DB
	cache  map[string]shapes.Shape
	closed bool
}

// Open connects to the database at url.
func Open(ctx context.Context, url string) (*DB, error) {
	conn, err := sql.Open("postgres", url)
	if err != nil {
		return nil, fmt.Errorf("connect: %w", err)
	}
	if err := conn.PingContext(ctx); err != nil {
		conn.Close()
		return nil, err
	}
	return &DB{conn: conn, cache: make(map[string]shapes.Shape)}, nil
}

// Save stores a shape under its name.
func (db *DB) Save(ctx context.Context, s shapes.Shape) error {
	db.mu.Lock()
	defer db.mu.Unlock()
	if db.closed {
		return ErrClosed
	}
	body, err := json.Marshal(s)
	if err != nil {
		return err
	}
	if _, err := db.conn.ExecContext(ctx, "INSERT INTO shapes (name, body) VALUES ($1, $2)", s.String(), body); err != nil {
		return fmt.Errorf("save %s: %w", s, err)
	}
	db.cache[s.String()] = s
	return nil
}

// Load reads a shape back.
func (db *DB) Load(ctx context.Context, name string) (shapes.Shape, error) {
	if s, ok := db.cache[name]; ok {
		return s, nil
	}
	var body []byte
	err := db.conn.QueryRowContext(ctx, "SELECT body FROM shapes WHERE name = $1", name).Scan(&body)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, &NotFoundError{Key: name}
	} else if err != nil {
		return nil, err
	}
	var c shapes.Circle
	if err := json.Unmarshal(body, &c); err != nil {
		return nil, err
	}
	return c, nil
}

// Export writes every cached shape to a file.
func (db *DB) Export(path string) (err error) {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer func() {
		if cerr := f.Close(); err == nil {
			err = cerr
		}
	}()
	enc := json.NewEncoder(f)
	for name, s := range db.cache {
		if err := enc.Encode(map[string]any{"name": name, "area": s.Area()}); err != nil {
			return err
		}
	}
	return nil
}

// MustLoad panics when the shape is missing.
func (db *DB) MustLoad(name string) shapes.Shape {
	s, err := db.Load(context.Background(), name)
	if err != nil {
		panic(err)
	}
	return s
}

// Close releases the connection.
func (db *DB) Close() error {
	db.mu.Lock()
	defer db.mu.Unlock()
	db.closed = true
	return db.conn.Close()
}

// safely runs f and turns a panic into an error.
func safely(f func()) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("recovered: %v", r)
		}
	}()
	f()
	return nil
}
