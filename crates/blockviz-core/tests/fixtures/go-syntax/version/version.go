package version

// Version of the tour.
const Version = "1.2.3"

func init() {
	if Version == "" {
		panic("no version")
	}
}
