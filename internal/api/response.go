package api

import (
	"encoding/json"
	"net/http"
)

// ProblemDetail represents an RFC 7807 Problem Details response.
// See: https://datatracker.ietf.org/doc/html/rfc7807
type ProblemDetail struct {
	Type     string `json:"type"`
	Title    string `json:"title"`
	Status   int    `json:"status"`
	Detail   string `json:"detail,omitempty"`
	Instance string `json:"instance,omitempty"`
}

// Problem types (relative paths, resolved against API base URL).
const (
	ProblemTypeNotFound   = "/problems/not-found"
	ProblemTypeBadRequest = "/problems/bad-request"
)

// writeJSON writes a JSON response with the given status code.
func writeJSON(w http.ResponseWriter, logger logger, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	if err := json.NewEncoder(w).Encode(v); err != nil {
		logger.Error("encode response", "error", err)
	}
}

// writeError writes an RFC 7807 Problem Details error response.
func writeError(w http.ResponseWriter, logger logger, status int, problemType, title, detail string) {
	problem := ProblemDetail{
		Type:   problemType,
		Title:  title,
		Status: status,
		Detail: detail,
	}
	writeJSON(w, logger, status, problem)
}

// writeNotFound writes a 404 Not Found error response.
func writeNotFound(w http.ResponseWriter, logger logger, detail string) {
	writeError(w, logger, http.StatusNotFound, ProblemTypeNotFound, "Not Found", detail)
}

// writeBadRequest writes a 400 Bad Request error response.
func writeBadRequest(w http.ResponseWriter, logger logger, detail string) {
	writeError(w, logger, http.StatusBadRequest, ProblemTypeBadRequest, "Bad Request", detail)
}

// logger is a minimal interface for logging errors.
type logger interface {
	Error(msg string, args ...any)
}
