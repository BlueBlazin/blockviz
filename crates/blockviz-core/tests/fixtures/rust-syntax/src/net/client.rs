use super::encode;
use crate::Result;
use tokio::net::TcpStream;
use tokio::io::AsyncWriteExt;

/// Talks to one server.
pub struct Client {
    stream: TcpStream,
}

impl Client {
    pub async fn connect(addr: &str) -> Result<Client> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Client { stream })
    }

    pub async fn send(&mut self, message: &str) -> Result<()> {
        let frame = encode(message);
        self.stream.write_all(&frame).await?;
        Ok(())
    }
}
