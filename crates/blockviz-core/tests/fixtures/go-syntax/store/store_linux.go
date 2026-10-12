package store

import "golang.org/x/sys/unix"

// lockFile takes an advisory lock on the database file.
func lockFile(fd int) error {
	return unix.Flock(fd, unix.LOCK_EX)
}
