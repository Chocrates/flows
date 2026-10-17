// Command qctl is the operator CLI for a running server.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"text/tabwriter"
	"time"
)

type client struct {
	base   string
	tenant string
	http   *http.Client
}

func (c *client) do(method, path string, out any) error {
	req, err := http.NewRequest(method, strings.TrimRight(c.base, "/")+path, nil)
	if err != nil {
		return err
	}
	req.Header.Set("X-Tenant", c.tenant)
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return fmt.Errorf("%s %s: %s: %s", method, path, resp.Status, strings.TrimSpace(string(b)))
	}
	if out == nil {
		return nil
	}
	return json.NewDecoder(resp.Body).Decode(out)
}

const usage = `usage: qctl [-addr URL] [-tenant T] <command> [args]

commands:
  stats                     counts by state
  ls [state] [kind]         list jobs
  show <id>                 print one job as JSON
  replay <id>               move a parked job back to pending
  purge [older-than]        delete parked jobs (default 168h)
`

func main() {
	addr := flag.String("addr", envOr("QCTL_ADDR", "http://localhost:8080"), "server base URL")
	tenant := flag.String("tenant", os.Getenv("QCTL_TENANT"), "tenant to act as")
	flag.Usage = func() { fmt.Fprint(os.Stderr, usage) }
	flag.Parse()
	c := &client{base: *addr, tenant: *tenant, http: &http.Client{Timeout: 15 * time.Second}}
	if err := dispatch(c, flag.Args()); err != nil {
		fmt.Fprintln(os.Stderr, "qctl:", err)
		os.Exit(1)
	}
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

func dispatch(c *client, args []string) error {
	if len(args) == 0 {
		flag.Usage()
		return fmt.Errorf("no command")
	}
	switch args[0] {
	case "stats":
		var counts map[string]int
		if err := c.do("GET", "/v1/stats", &counts); err != nil {
			return err
		}
		tw := tabwriter.NewWriter(os.Stdout, 0, 4, 2, ' ', 0)
		for state, n := range counts {
			fmt.Fprintf(tw, "%s\t%d\n", state, n)
		}
		return tw.Flush()
	case "ls":
		q := url.Values{}
		if len(args) > 1 {
			q.Set("state", args[1])
		}
		if len(args) > 2 {
			q.Set("kind", args[2])
		}
		var rows []map[string]any
		if err := c.do("GET", "/v1/jobs?"+q.Encode(), &rows); err != nil {
			return err
		}
		tw := tabwriter.NewWriter(os.Stdout, 0, 4, 2, ' ', 0)
		fmt.Fprintln(tw, "ID\tKIND\tSTATE\tATTEMPTS\tLAST ERROR")
		for _, r := range rows {
			fmt.Fprintf(tw, "%v\t%v\t%v\t%v\t%.60v\n", r["id"], r["kind"], r["state"], r["attempts"], r["last_error"])
		}
		return tw.Flush()
	case "show", "replay":
		if len(args) < 2 {
			return fmt.Errorf("%s needs a job id", args[0])
		}
		var out json.RawMessage
		method, path := "GET", "/v1/jobs/"+args[1]
		if args[0] == "replay" {
			method, path = "POST", "/v1/dead/"+args[1]+"/replay"
		}
		if err := c.do(method, path, &out); err != nil {
			return err
		}
		fmt.Println(string(out))
		return nil
	case "purge":
		age := "168h"
		if len(args) > 1 {
			age = args[1]
		}
		var out map[string]int
		if err := c.do("DELETE", "/v1/dead?older_than="+url.QueryEscape(age), &out); err != nil {
			return err
		}
		fmt.Printf("deleted %d parked jobs\n", out["deleted"])
		return nil
	default:
		return fmt.Errorf("unknown command %q", args[0])
	}
}
