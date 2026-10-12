//go:build !windows

// Package netx talks to the shape service.
package netx

import (
	"bufio"
	"io"
	"net"
	"net/http"
	"time"
)

// Client is a line-oriented connection.
type Client struct {
	conn net.Conn
	r    *bufio.Reader
	http *http.Client
}

// Dial connects to addr.
func Dial(addr string) (*Client, error) {
	conn, err := net.DialTimeout("tcp", addr, 5*time.Second)
	if err != nil {
		return nil, err
	}
	return &Client{conn: conn, r: bufio.NewReader(conn), http: &http.Client{Timeout: time.Minute}}, nil
}

// Ask sends a line and reads the answer.
func (c *Client) Ask(line string) (string, error) {
	if _, err := io.WriteString(c.conn, line+"\n"); err != nil {
		return "", err
	}
	return c.r.ReadString('\n')
}

// Fetch downloads a document.
func (c *Client) Fetch(url string) ([]byte, error) {
	resp, err := c.http.Get(url)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	return io.ReadAll(resp.Body)
}

// Pump copies messages until the channel closes or the timeout passes.
func Pump(in <-chan string, out chan<- string, quit chan struct{}) int {
	n := 0
	for {
		select {
		case msg, ok := <-in:
			if !ok {
				return n
			}
			out <- msg
			n++
		case <-time.After(time.Second):
			return n
		case <-quit:
			return n
		}
	}
}
