package api

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestWriteJSON(t *testing.T) {
	t.Parallel()

	rec := httptest.NewRecorder()
	log := newDiscardLogger()
	data := map[string]string{"foo": "bar"}

	writeJSON(rec, log, http.StatusCreated, data)

	if rec.Code != http.StatusCreated {
		t.Errorf("status = %d, want %d", rec.Code, http.StatusCreated)
	}
	assertJSONContentType(t, rec)

	body, _ := io.ReadAll(rec.Body)
	want := `{"foo":"bar"}` + "\n"
	if string(body) != want {
		t.Errorf("body = %q, want %q", string(body), want)
	}
}

func TestWriteError(t *testing.T) {
	t.Parallel()

	rec := httptest.NewRecorder()
	log := newDiscardLogger()

	writeError(rec, log, http.StatusNotFound, ProblemTypeNotFound, "Not Found", "Agent 'foo' was not found")

	if rec.Code != http.StatusNotFound {
		t.Errorf("status = %d, want %d", rec.Code, http.StatusNotFound)
	}
	assertJSONContentType(t, rec)

	var problem ProblemDetail
	if err := json.NewDecoder(rec.Body).Decode(&problem); err != nil {
		t.Fatalf("decode response: %v", err)
	}

	if problem.Type != "/problems/not-found" {
		t.Errorf("type = %q, want %q", problem.Type, "/problems/not-found")
	}
	if problem.Title != "Not Found" {
		t.Errorf("title = %q, want %q", problem.Title, "Not Found")
	}
	if problem.Status != http.StatusNotFound {
		t.Errorf("status = %d, want %d", problem.Status, http.StatusNotFound)
	}
	if problem.Detail != "Agent 'foo' was not found" {
		t.Errorf("detail = %q, want %q", problem.Detail, "Agent 'foo' was not found")
	}
}

func TestWriteNotFound(t *testing.T) {
	t.Parallel()

	rec := httptest.NewRecorder()
	log := newDiscardLogger()

	writeNotFound(rec, log, "Resource not found")

	if rec.Code != http.StatusNotFound {
		t.Errorf("status = %d, want %d", rec.Code, http.StatusNotFound)
	}

	var problem ProblemDetail
	if err := json.NewDecoder(rec.Body).Decode(&problem); err != nil {
		t.Fatalf("decode response: %v", err)
	}

	if problem.Status != http.StatusNotFound {
		t.Errorf("status = %d, want %d", problem.Status, http.StatusNotFound)
	}
}

func TestWriteBadRequest(t *testing.T) {
	t.Parallel()

	rec := httptest.NewRecorder()
	log := newDiscardLogger()

	writeBadRequest(rec, log, "Invalid input")

	if rec.Code != http.StatusBadRequest {
		t.Errorf("status = %d, want %d", rec.Code, http.StatusBadRequest)
	}

	var problem ProblemDetail
	if err := json.NewDecoder(rec.Body).Decode(&problem); err != nil {
		t.Fatalf("decode response: %v", err)
	}

	if problem.Status != http.StatusBadRequest {
		t.Errorf("status = %d, want %d", problem.Status, http.StatusBadRequest)
	}
}
