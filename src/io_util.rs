use std::io::BufRead;


pub(crate) trait BufReadExt {
    fn peek(&mut self) -> Result<Option<u8>, std::io::Error>;
    fn read_byte(&mut self) -> Result<Option<u8>, std::io::Error>;
}
impl<R: BufRead> BufReadExt for R {
    fn peek(&mut self) -> Result<Option<u8>, std::io::Error> {
        self.fill_buf()
            .map(|buf|
                buf.get(0)
                    .map(|b| *b)
            )
    }

    fn read_byte(&mut self) -> Result<Option<u8>, std::io::Error> {
        match self.peek() {
            Ok(Some(b)) => {
                self.consume(1);
                Ok(Some(b))
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}


pub(crate) trait IoResultOptionExt<T> {
    fn unwrap_eof(self) -> Result<T, std::io::Error>;
}
impl<T> IoResultOptionExt<T> for Result<Option<T>, std::io::Error> {
    fn unwrap_eof(self) -> Result<T, std::io::Error> {
        match self {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(std::io::ErrorKind::UnexpectedEof.into()),
            Err(e) => Err(e),
        }
    }
}
