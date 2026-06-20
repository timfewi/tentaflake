package config

import (
	"os"
	"strings"
	"testing"
)

// setenv sets an env var and registers a cleanup to restore the original value.
func setenv(t *testing.T, key, value string) {
	t.Helper()
	prev, wasSet := os.LookupEnv(key)
	if err := os.Setenv(key, value); err != nil {
		t.Fatalf("Setenv(%q, %q): %v", key, value, err)
	}
	t.Cleanup(func() {
		if wasSet {
			os.Setenv(key, prev)
		} else {
			os.Unsetenv(key)
		}
	})
}

// unsetenv unsets an env var and registers a cleanup to restore the original value.
func unsetenv(t *testing.T, key string) {
	t.Helper()
	prev, wasSet := os.LookupEnv(key)
	os.Unsetenv(key)
	t.Cleanup(func() {
		if wasSet {
			os.Setenv(key, prev)
		}
	})
}

func TestEnvStr(t *testing.T) {
	tests := []struct {
		name     string
		key      string
		def      string
		envVal   string // empty means unset
		expected string
	}{
		{
			name:     "returns env value when set",
			key:      "TEST_ENV_STR_VAL",
			def:      "default",
			envVal:   "custom-value",
			expected: "custom-value",
		},
		{
			name:     "returns default when env unset",
			key:      "TEST_ENV_STR_UNSET",
			def:      "fallback",
			envVal:   "",
			expected: "fallback",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			unsetenv(t, tc.key)
			if tc.envVal != "" {
				setenv(t, tc.key, tc.envVal)
			}
			got := envStr(tc.key, tc.def)
			if got != tc.expected {
				t.Errorf("envStr(%q, %q) = %q; want %q", tc.key, tc.def, got, tc.expected)
			}
		})
	}
}

func TestEnvInt(t *testing.T) {
	tests := []struct {
		name        string
		key         string
		def         int
		envVal      string // empty means unset
		expectedVal int
		expectErr   bool
	}{
		{
			name:        "returns int value when valid env set",
			key:         "TEST_ENV_INT_VAL",
			def:         42,
			envVal:      "99",
			expectedVal: 99,
			expectErr:   false,
		},
		{
			name:        "returns default when env unset",
			key:         "TEST_ENV_INT_UNSET",
			def:         42,
			envVal:      "",
			expectedVal: 42,
			expectErr:   false,
		},
		{
			name:        "returns error when env has invalid value",
			key:         "TEST_ENV_INT_BAD",
			def:         42,
			envVal:      "not-a-number",
			expectedVal: 0,
			expectErr:   true,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			unsetenv(t, tc.key)
			if tc.envVal != "" {
				setenv(t, tc.key, tc.envVal)
			}
			got, err := envInt(tc.key, tc.def)
			if tc.expectErr {
				if err == nil {
					t.Errorf("envInt(%q, %d) expected error, got nil", tc.key, tc.def)
				}
				return
			}
			if err != nil {
				t.Errorf("envInt(%q, %d) unexpected error: %v", tc.key, tc.def, err)
				return
			}
			if got != tc.expectedVal {
				t.Errorf("envInt(%q, %d) = %d; want %d", tc.key, tc.def, got, tc.expectedVal)
			}
		})
	}
}

func TestLoad(t *testing.T) {
	tests := []struct {
		name        string
		env         map[string]string // key→value; keys missing from map are unset
		want        *Config
		wantErr     bool
		errContains string
	}{
		{
			name: "all defaults when no env vars set",
			env:  map[string]string{},
			want: &Config{
				Port:           9090,
				DBPath:         "/var/lib/hermes-audit/events.db",
				WatchDirs:      nil,
				RetentionHours: 24,
			},
			wantErr: false,
		},
		{
			name: "custom port via AUDIT_PORT",
			env: map[string]string{
				"AUDIT_PORT": "8080",
			},
			want: &Config{
				Port:           8080,
				DBPath:         "/var/lib/hermes-audit/events.db",
				WatchDirs:      nil,
				RetentionHours: 24,
			},
			wantErr: false,
		},
		{
			name: "port 0 out of range returns error",
			env: map[string]string{
				"AUDIT_PORT": "0",
			},
			want:        nil,
			wantErr:     true,
			errContains: "out of range",
		},
		{
			name: "port 65536 out of range returns error",
			env: map[string]string{
				"AUDIT_PORT": "65536",
			},
			want:        nil,
			wantErr:     true,
			errContains: "out of range",
		},
		{
			name: "custom DBPath via AUDIT_DB_PATH",
			env: map[string]string{
				"AUDIT_DB_PATH": "/custom/path/db.sqlite",
			},
			want: &Config{
				Port:           9090,
				DBPath:         "/custom/path/db.sqlite",
				WatchDirs:      nil,
				RetentionHours: 24,
			},
			wantErr: false,
		},
		{
			name: "custom retention via AUDIT_RETENTION_HOURS",
			env: map[string]string{
				"AUDIT_RETENTION_HOURS": "72",
			},
			want: &Config{
				Port:           9090,
				DBPath:         "/var/lib/hermes-audit/events.db",
				WatchDirs:      nil,
				RetentionHours: 72,
			},
			wantErr: false,
		},
		{
			name: "WatchDirs parsing via AUDIT_WATCH_DIRS",
			env: map[string]string{
				"AUDIT_WATCH_DIRS": "/dir1, /dir2 ,/dir3",
			},
			want: &Config{
				Port:           9090,
				DBPath:         "/var/lib/hermes-audit/events.db",
				WatchDirs:      []string{"/dir1", "/dir2", "/dir3"},
				RetentionHours: 24,
			},
			wantErr: false,
		},
		{
			name: "empty AUDIT_WATCH_DIRS yields nil WatchDirs",
			env: map[string]string{
				"AUDIT_WATCH_DIRS": "",
			},
			want: &Config{
				Port:           9090,
				DBPath:         "/var/lib/hermes-audit/events.db",
				WatchDirs:      nil,
				RetentionHours: 24,
			},
			wantErr: false,
		},
		{
			name: "invalid retention zero returns error",
			env: map[string]string{
				"AUDIT_RETENTION_HOURS": "0",
			},
			want:        nil,
			wantErr:     true,
			errContains: "must be >= 1",
		},
		{
			name: "invalid retention negative returns error",
			env: map[string]string{
				"AUDIT_RETENTION_HOURS": "-5",
			},
			want:        nil,
			wantErr:     true,
			errContains: "must be >= 1",
		},
		{
			name: "invalid AUDIT_PORT non-numeric returns error",
			env: map[string]string{
				"AUDIT_PORT": "abc",
			},
			want:        nil,
			wantErr:     true,
			errContains: "AUDIT_PORT",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			// Save and restore all relevant env vars.
			for _, k := range []string{"AUDIT_PORT", "AUDIT_DB_PATH", "AUDIT_WATCH_DIRS", "AUDIT_RETENTION_HOURS"} {
				unsetenv(t, k)
			}
			for k, v := range tc.env {
				setenv(t, k, v)
			}

			got, err := Load()

			if tc.wantErr {
				if err == nil {
					t.Fatalf("Load() expected error, got nil")
				}
				if tc.errContains != "" && !strings.Contains(err.Error(), tc.errContains) {
					t.Errorf("Load() error = %q; want error containing %q", err.Error(), tc.errContains)
				}
				return
			}

			if err != nil {
				t.Fatalf("Load() unexpected error: %v", err)
			}

			if got.Port != tc.want.Port {
				t.Errorf("Load().Port = %d; want %d", got.Port, tc.want.Port)
			}
			if got.DBPath != tc.want.DBPath {
				t.Errorf("Load().DBPath = %q; want %q", got.DBPath, tc.want.DBPath)
			}
			if got.RetentionHours != tc.want.RetentionHours {
				t.Errorf("Load().RetentionHours = %d; want %d", got.RetentionHours, tc.want.RetentionHours)
			}

			// Compare WatchDirs
			wl, gl := len(tc.want.WatchDirs), len(got.WatchDirs)
			if wl != gl {
				t.Errorf("Load().WatchDirs len = %d; want %d", gl, wl)
				t.Logf("  got  WatchDirs: %v", got.WatchDirs)
				t.Logf("  want WatchDirs: %v", tc.want.WatchDirs)
			} else {
				for i := range got.WatchDirs {
					if got.WatchDirs[i] != tc.want.WatchDirs[i] {
						t.Errorf("Load().WatchDirs[%d] = %q; want %q", i, got.WatchDirs[i], tc.want.WatchDirs[i])
					}
				}
			}
		})
	}
}

