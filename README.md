# Amalthea

## About

Experimental kernel for Jupyter and Myriac, written in Rust. 

![image](https://user-images.githubusercontent.com/470418/151626974-52ac0047-0e98-494d-ad00-c0d293df696f.png)

During the prototyping phase, this kernel will implement a trivial echo-based language. Later, it will be factored into a [Rust Crate](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html) library that has no language implementation at all, but provides the shared Jupyter functionality necessary to build Rust-based kernels for other languages. This crate will then be used as the basis for Python and R kernels. 

### Why not Xeus?

The [Xeus](https://github.com/jupyter-xeus/xeus) project supplies all the nuts and bolts of Jupyter kernel communication, with the goal of letting kernel implementors focus only on the actual language bindings. 

Unfortunately this project cuts across architectural boundaries in ways that make it hard to extend with Rust. For example, Xeus depends on an army of C/C++ libraries, some header-only, that provide its JSON and ZeroMQ functionality; consequently extending it with Rust requires either marshaling structured data across the language boundary (difficult and tedious) or using multiple, possibly incompatible, libraries that serve the same purpose in the same binary.

Building in pure Rust dramatically simplifies the development environment and lets us standardize on idiomatic Rust tools like `serde_json`. It also shortens the distance to compiling for WASM, a door we'd like to leave open for investigation into browser-only versions of Myriac (a la vscode.dev).

### Implemented Features

- [X] Jupyter protocol implementation via ZeroMQ
- [X] Type-safe Rust structures/enums for (subset of) Jupyter messages
- [X] Heartbeats
- [X] Shell, iopub, and control sockets
- [X] Replies to kernel info request (returns echo language)
- [X] HMAC signature validation on messages
- [X] Execution counter
- [X] Handle completion requests/replies
- [X] Simple "execution" that echoes input

### Up Next

- [ ] Errors forwarded to client/front end
- [ ] display_data message: plots, data
- [ ] Standard output & standard error forwarding
- [ ] Refactor Echo language out into stubs to be implemented by other languages
- [ ] Produce a crate instead of a binary with an entry point

### What's with the name?

This is a Jupyter kernel; Amalthea is [one of Jupiter's moons](https://en.wikipedia.org/wiki/Amalthea_(moon)).

### Installation/Usage

Assuming you have a working Rust toolchain, first build the sources, then install the kernelspec. From the repository root:

```bash
$ cargo build
$ ./target/debug/amalthea -- install
```

This installs a JSON file to the Jupyter kernel registry. After it completes, the Amalthea kernel will be available on all Jupyter frontends on your system (Notebook, Lab, Myriac, etc.).

Set the environment variable `RUST_LOG=trace` to get detailed diagnostic output if desired; more fine-grained control is available as documented in [env_logger](https://docs.rs/env_logger/0.9.0/env_logger/#enabling-logging).

## Related

[Xeus](https://github.com/jupyter-xeus/xeus), a C++ base/reference kernel implementation

[IRKernel](https://github.com/IRkernel/IRkernel), a kernel for R written primarily in R itself

[EvCxR Kernel](https://github.com/google/evcxr/tree/main/evcxr_jupyter), a kernel for Rust written in Rust

[Myriac Console](https://github.com/rstudio/myriac-console), an experimental Jupyter front end


