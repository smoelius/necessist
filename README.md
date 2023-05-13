# Necessist

Run tests with statements and method calls removed to help identify broken tests

Necessist currently supports Foundry, Golang, Hardhat TS, and Rust.

**Contents**

- [Installation](#installation)
- [Overview](#overview)
- [Usage](#usage)
- [Details](#details)
- [Configuration files](#configuration-files-experimental)
- [Goals](#goals)
- [Limitations](#limitations)
- [License](#license)

## Installation

#### System requirements:

Install `pkg-config` and `sqlite3` development files on your system, e.g., on Ubuntu:

```sh
sudo apt install pkg-config libsqlite3-dev
```

#### Install Necessist from [crates.io]:

```sh
cargo install necessist --version=^0.1.0-beta
```

#### Install Necessist from [github.com]:

```sh
cargo install --git https://github.com/trailofbits/necessist --branch release
```

## Overview

Necessist iteratively removes statements and method calls from tests and then runs them. If a test passes with a statement or method call removed, it could indicate a problem in the test. Or worse, it could indicate a problem in the code being tested.

### Example

This example is from [`rust-openssl`]. The `verify_untrusted_callback_override_ok` test checks that a failed certificate validation can be overridden by a callback. But if the callback were never called (e.g., because of a failed connection), the test would still pass. Necessist reveals this fact by showing that the test passes without the call to `set_verify_callback`:

```rust
#[test]
fn verify_untrusted_callback_override_ok() {
    let server = Server::builder().build();

    let mut client = server.client();
    client
        .ctx()
        .set_verify_callback(SslVerifyMode::PEER, |_, x509| { //
            assert!(x509.current_cert().is_some());           // Test passes without this call
            true                                              // to `set_verify_callback`.
        });                                                   //

    client.connect();
}
```

Following this discovery, a flag was [added to the test] to record whether the callback is called. The flag must be set for the test to succeed:

```rust
#[test]
fn verify_untrusted_callback_override_ok() {
    static CALLED_BACK: AtomicBool = AtomicBool::new(false);  // Added

    let server = Server::builder().build();

    let mut client = server.client();
    client
        .ctx()
        .set_verify_callback(SslVerifyMode::PEER, |_, x509| {
            CALLED_BACK.store(true, Ordering::SeqCst);        // Added
            assert!(x509.current_cert().is_some());
            true
        });

    client.connect();
    assert!(CALLED_BACK.load(Ordering::SeqCst));              // Added
}
```

### Comparison to conventional mutation testing

Conventional mutation testing tries to identify _gaps in test coverage_, whereas Necessist tries to identify _bugs in existing tests_.

Conventional mutation testing tools (such a [`universalmutator`]) randomly inject faults into source code, and see whether the code's tests still pass. If they do, it could mean the code's tests are inadequate.

Notably, conventional mutation testing is about finding deficiencies in the set of tests as a whole, not in individual tests. That is, for any given test, randomly injecting faults into the code is not especially likely to reveal bugs in that test. This is unfortunate since some tests are more important than others, e.g., because ensuring the correctness of some parts of the code is more important than others.

By comparison, Necessist's approach of iteratively removing statements and method calls does target individual tests, and thus can reveal bugs in individual tests.

Of course, there is overlap is the sets of problems the two approaches can uncover, e.g., a failure to find an injected fault could indicate a bug in a test. Nonetheless, for the reasons just given, we see the two approaches as complementary, not competing.

## Usage

```
Usage: necessist [OPTIONS] [TEST_FILES]... [-- <ARGS>...]

Arguments:
  [TEST_FILES]...  Test files to mutilate (optional)
  [ARGS]...        Additional arguments to pass to each test command

Options:
      --allow <WARNING>        Silence <WARNING>; `--allow all` silences all warnings
      --default-config         Create a default necessist.toml file in the project's root directory (experimental)
      --deny <WARNING>         Treat <WARNING> as an error; `--deny all` treats all warnings as errors
      --dump                   Dump sqlite database contents to the console
      --dump-candidates        Dump removal candidates and exit (for debugging)
      --framework <FRAMEWORK>  Assume testing framework is <FRAMEWORK> [possible values: auto, foundry, hardhat-ts, rust]
      --no-dry-run             Do not perform dry runs
      --no-sqlite              Do not output to an sqlite database
      --quiet                  Do not output to the console
      --reset                  Discard sqlite database contents
      --resume                 Resume from the sqlite database
      --root <ROOT>            Root directory of the project under test
      --timeout <TIMEOUT>      Maximum number of seconds to run any test; 60 is the default, 0 means no timeout
      --verbose                Show test outcomes besides `passed`
  -h, --help                   Print help
  -V, --version                Print version
```

### Output

By default, Necessist outputs to the console only when tests pass. Passing `--verbose` causes Necessist to instead output all of the removal outcomes below.

| Outcome                                      | Meaning (With the statement/method call removed...) |
| -------------------------------------------- | --------------------------------------------------- |
| <span style="color:red">passed</span>        | The test(s) built and passed.                       |
| <span style="color:yellow">timed-out</span>  | The test(s) built but timed-out.                    |
| <span style="color:green">failed</span>      | The test(s) built but failed.                       |
| <span style="color:blue">nonbuildable</span> | The test(s) did not build.                          |

By default, Necessist outputs to both the console and to an sqlite database. For the latter, a tool like [sqlitebrowser](https://sqlitebrowser.org/) can be used to filter/sort the results.

## Details

Generally speaking, Necessist will not attempt to remove a statement if it is one the following:

- A statement containing other statements (e.g., a `for` loop)
- A statement consisting of a single method call
- A declaration (e.g., a local or `let` binding)
- A `break`, `continue`, or `return`

Also, for some frameworks, certain statements and methods are ignored. Click on a framework to see its specifics.

<details>
<summary>Foundry</summary>

In addition to the below, the Foundry framework ignores:

- the last statement in a function body
- a statement immediately following a use of `vm.prank` or any form of `vm.expect` (e.g., `vm.expectRevert`)
- an `emit` statement

#### Ignored functions

- Anything beginning with `assert` (e.g., `assertEq`)

#### Ignored methods

- `expectEmit`
- `expectRevert`
- `prank`
- `startPrank`
- `stopPrank`

</details>

<details>
<summary>Golang</summary>

In addition to the below, the Golang framework ignores:

- Any method call whose receiver is `assert` (e.g., `assert.Equal`)
- Any method call whose receiver is `require` (e.g., `require.Equal`)
- `defer` statements

#### Ignored methods\*

- `Close`
- `Error`
- `Errorf`
- `Fail`
- `FailNow`
- `Fatal`
- `Fatalf`
- `Log`
- `Logf`
- `Parallel`

\* This list is based primarily on [`testing.T`]'s methods. However, some methods with commonplace names are omitted to avoid colliding with other types' methods.

</details>

<details>
<summary>Hardhat TS</summary>

#### Ignored functions

- Anything beginning with `assert` (e.g., `assert.equal`)
- `expect`

#### Ignored methods

- Anything beginning with `should` (e.g., `should.equal`)
- Anything beginning with `to` (e.g., `to.equal`)
- `toNumber`
- `toString`

</details>

<details>
<summary>Rust</summary>

#### Ignored macros

- `assert`
- `assert_eq`
- `assert_matches`
- `assert_ne`
- `eprint`
- `eprintln`
- `panic`
- `print`
- `println`
- `unimplemented`
- `unreachable`

#### Ignored methods\*

- `as_bytes`
- `as_mut`
- `as_mut_os_str`
- `as_mut_os_string`
- `as_mut_slice`
- `as_mut_str`
- `as_os_str`
- `as_os_str_bytes`
- `as_path`
- `as_ref`
- `as_slice`
- `as_str`
- `borrow`
- `borrow_mut`
- `clone`
- `cloned`
- `copied`
- `deref`
- `deref_mut`
- `expect`
- `expect_err`
- `into_boxed_bytes`
- `into_boxed_os_str`
- `into_boxed_path`
- `into_boxed_slice`
- `into_boxed_str`
- `into_bytes`
- `into_os_string`
- `into_owned`
- `into_path_buf`
- `into_string`
- `into_vec`
- `iter`
- `iter_mut`
- `success`
- `to_os_string`
- `to_owned`
- `to_path_buf`
- `to_string`
- `to_vec`
- `unwrap`
- `unwrap_err`

\* This list is essentially the watched trait and inherent methods of Dylint's [`unnecessary_conversion_for_trait`] lint, with the following additions:

- `clone` (e.g. [`std::clone::Clone::clone`])
- `cloned` (e.g. [`std::iter::Iterator::cloned`])
- `copied` (e.g. [`std::iter::Iterator::copied`])
- `expect` (e.g. [`std::option::Option::expect`])
- `expect_err` (e.g. [`std::result::Result::expect_err`])
- `into_owned` (e.g. [`std::borrow::Cow::into_owned`])
- `success` (e.g. [`assert_cmd::assert::Assert::success`])
- `unwrap` (e.g. [`std::option::Option::unwrap`])
- `unwrap_err` (e.g. [`std::result::Result::unwrap_err`])

</details>

<p></p>

## Configuration files (experimental)

**Configuration files are experimental and their behavior could change at any time.**

A configuration file allows one to tailor Necessist's behavior with respect to a project. The file must be named `necessist.toml`, appear in the project's root directory, and be [toml] encoded. The file may contain one more of the options listed below.

### Hardhat TS configuration options

- `ignored_functions`: A list of strings. Functions whose names appear in the list are ignored.

### Rust configuration options

- `ignored_macros`: A list of strings. Macros whose names appear in the list are ignored.

## Limitations

- **Slow.** Modifying tests requires them to be rebuilt. Running Necessist on even moderately sized codebases can take several hours.

- **Triage requires intimate knowledge of the source code.** Generally speaking, Necessist does not produce "obvious" bugs. In our experience, deciding whether a statement/method call should be necessary requires intimate knowledge of the code under test. Necessist is best run on codebases for which one has (or intends to have) such knowledge.

## Goals

- If a project uses a supported framework, then `cd`ing into the project's directory and typing `necessist` (with no arguments) should produce meaningful output.

## References

- Groce, A., Ahmed, I., Jensen, C., McKenney, P.E., Holmes, J.: How verified (or tested) is my code? Falsification-driven verification and testing. Autom. Softw. Eng. **25**, 917–960 (2018). A [preprint] is available. See Section 2.3.

## License

Necessist is licensed and distributed under the AGPLv3 license. [Contact us](mailto:opensource@trailofbits.com) if you're looking for an exception to the terms.

[`assert_cmd::assert::assert::success`]: https://docs.rs/assert_cmd/latest/assert_cmd/assert/struct.Assert.html#method.success
[`rust-openssl`]: https://github.com/sfackler/rust-openssl
[`std::borrow::cow::into_owned`]: https://doc.rust-lang.org/std/borrow/enum.Cow.html#method.into_owned
[`std::clone::clone::clone`]: https://doc.rust-lang.org/std/clone/trait.Clone.html#tymethod.clone
[`std::iter::iterator::cloned`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.cloned
[`std::iter::iterator::copied`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.copied
[`std::option::option::expect`]: https://doc.rust-lang.org/std/option/enum.Option.html#method.expect
[`std::option::option::unwrap`]: https://doc.rust-lang.org/std/option/enum.Option.html#method.unwrap
[`std::result::result::expect_err`]: https://doc.rust-lang.org/std/result/enum.Result.html#method.expect_err
[`std::result::result::unwrap_err`]: https://doc.rust-lang.org/std/result/enum.Result.html#method.unwrap_err
[`testing.t`]: https://pkg.go.dev/testing#T
[`unnecessary_conversion_for_trait`]: https://github.com/trailofbits/dylint/tree/master/examples/supplementary/unnecessary_conversion_for_trait
[added to the test]: https://github.com/sfackler/rust-openssl/pull/1852
[crates.io]: https://crates.io/crates/necessist
[github.com]: https://github.com/trailofbits/necessist
[preprint]: https://agroce.github.io/asej18.pdf
[toml]: https://toml.io/en/
[`universalmutator`]: https://github.com/agroce/universalmutator
