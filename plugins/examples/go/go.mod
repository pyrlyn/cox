module github.com/pyrlyn/cox/plugins/examples/go

go 1.25

require github.com/pyrlyn/cox/plugins/sdk-go v0.0.0

require github.com/extism/go-pdk v1.1.3 // indirect

// The SDK in this repository, not a tagged release (T53.8 tags it).
replace github.com/pyrlyn/cox/plugins/sdk-go => ../../sdk-go
