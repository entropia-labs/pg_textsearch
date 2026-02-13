package main

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

//go:embed index.html
var indexHTML []byte

type Tenant struct {
	ID       int `json:"id"`
	DocCount int `json:"doc_count"`
}

type SearchResult struct {
	ID       int64   `json:"id"`
	DocID    int     `json:"doc_id"`
	ChunkID  int     `json:"chunk_id"`
	TenantID int     `json:"tenant_id"`
	Score    float64 `json:"score"`
	Content  string  `json:"content"`
}

type SearchResponse struct {
	Results  []SearchResult `json:"results"`
	QueryMs  float64        `json:"query_ms"`
	Query    string         `json:"query"`
	TenantID int            `json:"tenant_id"`
}

var pool *pgxpool.Pool

func main() {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		dsn = "postgres:///postgres"
	}

	var err error
	pool, err = pgxpool.New(context.Background(), dsn)
	if err != nil {
		log.Fatalf("Unable to connect to database: %v", err)
	}
	defer pool.Close()

	if err := pool.Ping(context.Background()); err != nil {
		log.Fatalf("Unable to ping database: %v", err)
	}

	http.HandleFunc("/", handleIndex)
	http.HandleFunc("/api/tenants", handleTenants)
	http.HandleFunc("/api/search", handleSearch)

	addr := ":8080"
	log.Printf("Listening on http://localhost%s", addr)
	log.Fatal(http.ListenAndServe(addr, nil))
}

func handleIndex(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Write(indexHTML)
}

func handleTenants(w http.ResponseWriter, r *http.Request) {
	rows, err := pool.Query(context.Background(),
		`SELECT tenant_id, count(*) AS doc_count
		 FROM tenant_docs
		 GROUP BY tenant_id
		 ORDER BY tenant_id`)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	defer rows.Close()

	var tenants []Tenant
	for rows.Next() {
		var t Tenant
		if err := rows.Scan(&t.ID, &t.DocCount); err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		tenants = append(tenants, t)
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(tenants)
}

func handleSearch(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query().Get("q")
	tenantStr := r.URL.Query().Get("tenant_id")

	if q == "" {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(SearchResponse{
			Results: []SearchResult{},
		})
		return
	}

	tenantID, err := strconv.Atoi(tenantStr)
	if err != nil {
		http.Error(w, "invalid tenant_id", 400)
		return
	}

	start := time.Now()

	rows, err := pool.Query(context.Background(),
		`SELECT id, doc_id, chunk_id, tenant_id,
		        content <@> to_bm25query($1,
		            'tenant_docs_bm25_idx') AS score,
		        left(content, 120) AS snippet
		 FROM tenant_docs
		 WHERE content <@> to_bm25query($1,
		           'tenant_docs_bm25_idx') < 0
		   AND tenant_id = $2
		 ORDER BY content <@> to_bm25query($1,
		              'tenant_docs_bm25_idx')
		 LIMIT 10`,
		q, tenantID)
	if err != nil {
		http.Error(w,
			fmt.Sprintf("query error: %v", err), 500)
		return
	}
	defer rows.Close()

	var results []SearchResult
	for rows.Next() {
		var sr SearchResult
		if err := rows.Scan(&sr.ID, &sr.DocID,
			&sr.ChunkID, &sr.TenantID,
			&sr.Score, &sr.Content); err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		results = append(results, sr)
	}

	elapsed := time.Since(start)

	if results == nil {
		results = []SearchResult{}
	}

	resp := SearchResponse{
		Results:  results,
		QueryMs:  float64(elapsed.Microseconds()) / 1000.0,
		Query:    q,
		TenantID: tenantID,
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(resp)
}
