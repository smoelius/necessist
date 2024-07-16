mod third_party_common;

const PATH: &str = "tests/third_party_tests/0";

#[cfg_attr(dylint_lib = "general", allow(non_thread_safe_call_in_test))]
#[test]
fn all_tests() {
    third_party_common::all_tests_in(PATH);
}

#[test]
fn stdout_files_are_sanitary() {
    third_party_common::stdout_files_are_sanitary_in(PATH);
}

#[test]
fn stdout_subsequence() {
    third_party_common::stdout_subsequence_in(PATH);
}
