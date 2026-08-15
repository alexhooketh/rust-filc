//! End-to-end attributed `extern "Fil-C"` example.

#![forbid(unsafe_code)]

pub mod bridge;

#[cfg(test)]
mod tests {
    use filc::Error;

    use super::bridge;

    #[test]
    fn extern_like_calls_copy_values_and_recover_after_a_filc_panic() {
        assert_eq!(bridge::add(20, 22).unwrap(), 42);
        assert_eq!(bridge::reverse(&[1, 2, 3, 4]).unwrap(), [4, 3, 2, 1]);
        assert_eq!(bridge::greet("Fil").unwrap(), "hello, Fil");

        let counter = bridge::counter_new(10).unwrap();
        assert_eq!(bridge::counter_add(&counter, 5).unwrap(), 15);
        let disposable = bridge::counter_new(99).unwrap();
        bridge::counter_drop(disposable).unwrap();

        assert!(matches!(
            bridge::trigger_oob(&[1, 2, 3]),
            Err(Error::HelperExited(_) | Error::Io(_))
        ));
        assert_eq!(bridge::add(1, 2).unwrap(), 3);
        assert!(matches!(
            bridge::counter_add(&counter, 1),
            Err(Error::WrongConnection)
        ));
    }
}
