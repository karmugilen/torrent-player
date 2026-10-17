//go:build !rustdump

package main

func rustDumpEnabled() bool { return false }

func rustEnsureDump(path string, force bool) error { return errRustUnavailable }

func rustLoadDump(path string) (*dumpIndex, error) { return nil, errRustUnavailable }

func rustLookup(hash string) (dumpRow, bool) { return dumpRow{}, false }

func rustDumpLen() int { return 0 }

func rustSeedAll(d *dashboard) int { return 0 }

func rustFetchCatalogPage(query string, size, after int) (*catalogPage, error) {
	return nil, errRustUnavailable
}

var errRustUnavailable = errString("rust dump not linked")

type errString string

func (e errString) Error() string { return string(e) }
