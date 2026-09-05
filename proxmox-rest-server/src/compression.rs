use anyhow::{Error, bail};
use hyper::header;

/// Possible Compression Methods, order determines preference (later is preferred)
#[derive(Eq, Ord, PartialEq, PartialOrd, Debug)]
pub enum CompressionMethod {
    Deflate,
    //    Gzip,
    //    Brotli,
}

impl CompressionMethod {
    pub fn content_encoding(&self) -> header::HeaderValue {
        header::HeaderValue::from_static(self.extension())
    }

    pub fn extension(&self) -> &'static str {
        match *self {
            //            CompressionMethod::Brotli => "br",
            //            CompressionMethod::Gzip => "gzip",
            CompressionMethod::Deflate => "deflate",
        }
    }
}

impl std::str::FromStr for CompressionMethod {
    type Err = Error;

    /// Parse one entry of an `Accept-Encoding` list, i.e. a coding with optional parameters.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (coding, parameters) = match s.split_once(';') {
            Some((coding, parameters)) => (coding.trim(), Some(parameters)),
            None => (s.trim(), None),
        };

        //            "br" => Ok(CompressionMethod::Brotli),
        //            "gzip" => Ok(CompressionMethod::Gzip),
        if !coding.eq_ignore_ascii_case("deflate") {
            bail!("unknown compression format");
        }

        for parameter in parameters.into_iter().flat_map(|p| p.split(';')) {
            let Some((name, value)) = parameter.split_once('=') else {
                continue;
            };

            // a weight of zero states that the client does not accept this coding at all
            if name.trim().eq_ignore_ascii_case("q") && !is_positive_qvalue(value.trim()) {
                bail!("compression format not accepted");
            }
        }

        Ok(CompressionMethod::Deflate)
    }
}

/// Whether the value is a qvalue greater than zero.
///
/// The grammar allows `0` and `1` with at most three fractional digits, and nothing else; an
/// unparsable weight is not a refusal, but it is not an acceptance either.
fn is_positive_qvalue(value: &str) -> bool {
    let (integer, fraction) = match value.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (value, ""),
    };

    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }

    match integer {
        "1" => fraction.bytes().all(|b| b == b'0'),
        "0" => fraction.bytes().any(|b| b != b'0'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weight_of_zero_means_not_acceptable() {
        for encoding in ["deflate;q=0", "deflate;q=0.0", "deflate;q=0.000"] {
            assert!(
                encoding.parse::<CompressionMethod>().is_err(),
                "accepted {encoding}"
            );
        }
    }

    #[test]
    fn a_positive_weight_is_accepted() {
        for encoding in ["deflate", "deflate;q=1", "deflate;q=0.5", "deflate;q=0.001"] {
            assert_eq!(
                encoding.parse::<CompressionMethod>().unwrap(),
                CompressionMethod::Deflate,
                "rejected {encoding}"
            );
        }
    }

    #[test]
    fn optional_whitespace_around_the_parameters_is_allowed() {
        for encoding in ["deflate ;q=0", "deflate; q=0", " deflate ; q = 0 "] {
            assert!(
                encoding.parse::<CompressionMethod>().is_err(),
                "accepted {encoding}"
            );
        }
        for encoding in ["deflate ;q=1", "DEFLATE", "deflate ; q = 0.5"] {
            assert_eq!(
                encoding.parse::<CompressionMethod>().unwrap(),
                CompressionMethod::Deflate,
                "rejected {encoding}"
            );
        }
    }

    #[test]
    fn a_weight_outside_the_grammar_is_not_an_acceptance() {
        for encoding in [
            "deflate;q=2",
            "deflate;q=1.5",
            "deflate;q=1e-3",
            "deflate;q=0.0001",
        ] {
            assert!(
                encoding.parse::<CompressionMethod>().is_err(),
                "accepted {encoding}"
            );
        }
    }

    #[test]
    fn other_encodings_are_not_accepted() {
        for encoding in ["gzip", "br", "deflate;q=", "identity"] {
            assert!(
                encoding.parse::<CompressionMethod>().is_err(),
                "accepted {encoding}"
            );
        }
    }
}
