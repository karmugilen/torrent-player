use bytes::Bytes;
use num_bigint::BigUint;
use rand::Rng;
use sha1::{Digest, Sha1};
use std::cmp::min;
use std::io::{self, Error, ErrorKind};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

pub const PRIME_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563";

pub const MAX_PAD_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CryptoMethod(pub u32);

impl CryptoMethod {
    pub const PLAINTEXT: CryptoMethod = CryptoMethod(1);
    pub const RC4: CryptoMethod = CryptoMethod(2);
    pub const ALL: CryptoMethod = CryptoMethod(3);

    pub fn supports_plaintext(&self) -> bool {
        (self.0 & Self::PLAINTEXT.0) != 0
    }

    pub fn supports_rc4(&self) -> bool {
        (self.0 & Self::RC4.0) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderObfuscationPolicy {
    pub require_preferred: bool,
    pub preferred: bool,
}

impl Default for HeaderObfuscationPolicy {
    fn default() -> Self {
        Self {
            require_preferred: false,
            preferred: true,
        }
    }
}

/// Pure-Rust RC4 implementation.
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        let mut rc4 = Self { s, i: 0, j: 0 };
        // Discard first 1024 bytes per MSE spec
        let mut burn = [0u8; 1024];
        rc4.apply(&mut burn);
        rc4
    }

    pub fn apply(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k =
                self.s[(self.s[self.i as usize].wrapping_add(self.s[self.j as usize])) as usize];
            *b ^= k;
        }
    }
}

fn prime_p() -> BigUint {
    BigUint::parse_bytes(PRIME_HEX.as_bytes(), 16).unwrap()
}

fn sha1_hash(parts: &[&[u8]]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    for p in parts {
        hasher.update(p);
    }
    let res = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&res);
    out
}

fn pad_to_96(val: &BigUint) -> [u8; 96] {
    let b = val.to_bytes_be();
    let mut out = [0u8; 96];
    if b.len() <= 96 {
        out[96 - b.len()..].copy_from_slice(&b);
    } else {
        out.copy_from_slice(&b[b.len() - 96..]);
    }
    out
}

fn random_pad(max_len: usize) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let len = rng.gen_range(0..=max_len);
    let mut buf = vec![0u8; len];
    rng.fill(&mut buf[..]);
    buf
}

pub fn default_crypto_selector(provided: CryptoMethod) -> CryptoMethod {
    if provided.supports_plaintext() {
        CryptoMethod::PLAINTEXT
    } else {
        CryptoMethod::RC4
    }
}

pub fn suffix_match_len(a: &[u8], b: &[u8]) -> usize {
    let mut max_cmp = b.len();
    if max_cmp > a.len() {
        max_cmp = a.len();
    }
    for i in (1..=max_cmp).rev() {
        if a[a.len() - i..] == b[..i] {
            return i;
        }
    }
    0
}

/// Read from `reader` until byte sequence `target` has been seen at the end of input.
pub async fn read_until<R: AsyncRead + Unpin>(
    reader: &mut R,
    target: &[u8],
    max_bytes: usize,
) -> io::Result<()> {
    let mut window = vec![0u8; target.len()];
    let mut total_read = 0;

    while total_read < max_bytes {
        let mut b = [0u8; 1];
        let n = reader.read(&mut b).await?;
        if n == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "EOF before match"));
        }
        total_read += 1;

        window.remove(0);
        window.push(b[0]);

        if suffix_match_len(&window, target) == target.len() {
            return Ok(());
        }
    }

    Err(Error::new(
        ErrorKind::NotFound,
        "target not found within max_bytes",
    ))
}

/// MSE Stream wrapper around an underlying `AsyncRead + AsyncWrite` stream.
pub struct MseStream<S> {
    stream: S,
    decrypt: Option<Rc4>,
    encrypt: Option<Rc4>,
    initial_read_buf: Bytes,
}

impl<S> MseStream<S> {
    pub fn new(stream: S, decrypt: Option<Rc4>, encrypt: Option<Rc4>, initial: Bytes) -> Self {
        Self {
            stream,
            decrypt,
            encrypt,
            initial_read_buf: initial,
        }
    }

    pub fn is_rc4(&self) -> bool {
        self.decrypt.is_some()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for MseStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.initial_read_buf.is_empty() {
            let to_copy = min(buf.remaining(), self.initial_read_buf.len());
            let slice = self.initial_read_buf.split_to(to_copy);
            buf.put_slice(&slice);
            return Poll::Ready(Ok(()));
        }

        let before_len = buf.filled().len();
        match Pin::new(&mut self.stream).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let after_len = buf.filled().len();
                let newly_read = after_len - before_len;
                if newly_read > 0 {
                    if let Some(ref mut dec) = self.decrypt {
                        let filled = buf.filled_mut();
                        dec.apply(&mut filled[before_len..after_len]);
                    }
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for MseStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(ref mut enc) = self.encrypt {
            let mut encrypted = buf.to_vec();
            enc.apply(&mut encrypted);
            Pin::new(&mut self.stream).poll_write(cx, &encrypted)
        } else {
            Pin::new(&mut self.stream).poll_write(cx, buf)
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// MSE Initiator Handshake.
pub async fn initiate_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    skey: &[u8],
    initial_payload: Option<&[u8]>,
    crypto_provides: CryptoMethod,
) -> io::Result<(MseStream<S>, CryptoMethod)> {
    let p = prime_p();
    let g = BigUint::from(2u32);

    let (x_bytes, pad_a) = {
        let mut rng = rand::thread_rng();
        let mut x = [0u8; 20];
        rng.fill(&mut x[..]);
        let pad = random_pad(MAX_PAD_LEN);
        (x, pad)
    };
    let x_a = BigUint::from_bytes_be(&x_bytes);
    let y_a = g.modpow(&x_a, &p);
    let y_a_bytes = pad_to_96(&y_a);

    // Send Y_A + PadA
    stream.write_all(&y_a_bytes).await?;
    stream.write_all(&pad_a).await?;
    stream.flush().await?;

    // Read Y_B (96 bytes)
    let mut y_b_bytes = [0u8; 96];
    stream.read_exact(&mut y_b_bytes).await?;
    let y_b = BigUint::from_bytes_be(&y_b_bytes);

    // Compute S
    let s = y_b.modpow(&x_a, &p);
    let s_bytes = pad_to_96(&s);

    // Send HASH('req1', S)
    let req1_hash = sha1_hash(&[b"req1", &s_bytes]);
    stream.write_all(&req1_hash).await?;

    // Send HASH('req2', SKEY) ^ HASH('req3', S)
    let req2_hash = sha1_hash(&[b"req2", skey]);
    let req3_hash = sha1_hash(&[b"req3", &s_bytes]);
    let mut xored = [0u8; 20];
    for i in 0..20 {
        xored[i] = req2_hash[i] ^ req3_hash[i];
    }
    stream.write_all(&xored).await?;

    // Prepare encrypted payload: VC (8 zeros) + crypto_provide (4 bytes) + pad_c_len (2 bytes) + pad_c + ia_len (2 bytes) + ia
    let key_a = sha1_hash(&[b"keyA", &s_bytes, skey]);
    let mut enc_a = Rc4::new(&key_a);

    let pad_c_len = {
        let mut rng = rand::thread_rng();
        rng.gen_range(0..=MAX_PAD_LEN) as u16
    };
    let pad_c = vec![0u8; pad_c_len as usize];

    let ia = initial_payload.unwrap_or(&[]);
    let ia_len = ia.len() as u16;

    let mut buf = Vec::with_capacity(8 + 4 + 2 + pad_c.len() + 2 + ia.len());
    buf.extend_from_slice(&[0u8; 8]); // VC
    buf.extend_from_slice(&crypto_provides.0.to_be_bytes());
    buf.extend_from_slice(&pad_c_len.to_be_bytes());
    buf.extend_from_slice(&pad_c);
    buf.extend_from_slice(&ia_len.to_be_bytes());
    buf.extend_from_slice(ia);

    enc_a.apply(&mut buf);
    stream.write_all(&buf).await?;
    stream.flush().await?;

    // KeyB for reading from receiver
    let key_b = sha1_hash(&[b"keyB", &s_bytes, skey]);
    let mut dec_b = Rc4::new(&key_b);

    // Synchronize on VC (8 zeros encrypted with KeyB)
    let mut expected_vc = [0u8; 8];
    dec_b.clone().apply(&mut expected_vc); // preview encrypted VC

    read_until(&mut stream, &expected_vc, 512 + 8).await?;

    // Advance dec_b past VC (8 bytes)
    let mut dummy_vc = [0u8; 8];
    dec_b.apply(&mut dummy_vc);

    // Read crypto_select (4 bytes) and pad_d_len (2 bytes)
    let mut hdr = [0u8; 6];
    stream.read_exact(&mut hdr).await?;
    dec_b.apply(&mut hdr);

    let crypto_select = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let pad_d_len = u16::from_be_bytes([hdr[4], hdr[5]]) as usize;

    if pad_d_len > 0 {
        let mut pad_d = vec![0u8; pad_d_len];
        stream.read_exact(&mut pad_d).await?;
        dec_b.apply(&mut pad_d);
    }

    let selected = CryptoMethod(crypto_select & crypto_provides.0);

    let (dec, enc) = if selected == CryptoMethod::RC4 {
        (Some(dec_b), Some(enc_a))
    } else {
        (None, None)
    };

    Ok((MseStream::new(stream, dec, enc, Bytes::new()), selected))
}

pub struct ReceiverResult<S> {
    pub stream: MseStream<S>,
    pub crypto_method: CryptoMethod,
    pub secret_key: Vec<u8>,
}

/// MSE Receiver Handshake.
pub async fn receive_handshake<S: AsyncRead + AsyncWrite + Unpin, I, F>(
    mut stream: S,
    skey_candidates: I,
    selector: F,
) -> io::Result<ReceiverResult<S>>
where
    I: IntoIterator<Item = Vec<u8>>,
    F: Fn(CryptoMethod) -> CryptoMethod,
{
    let p = prime_p();
    let g = BigUint::from(2u32);

    // Read Y_A (96 bytes)
    let mut y_a_bytes = [0u8; 96];
    stream.read_exact(&mut y_a_bytes).await?;
    let y_a = BigUint::from_bytes_be(&y_a_bytes);

    let (x_bytes, pad_b) = {
        let mut rng = rand::thread_rng();
        let mut x = [0u8; 20];
        rng.fill(&mut x[..]);
        let pad = random_pad(MAX_PAD_LEN);
        (x, pad)
    };
    let x_b = BigUint::from_bytes_be(&x_bytes);
    let y_b = g.modpow(&x_b, &p);
    let y_b_bytes = pad_to_96(&y_b);

    // Send Y_B + PadB
    stream.write_all(&y_b_bytes).await?;
    stream.write_all(&pad_b).await?;
    stream.flush().await?;

    // Compute S
    let s = y_a.modpow(&x_b, &p);
    let s_bytes = pad_to_96(&s);

    // Synchronize on HASH('req1', S) (20 bytes)
    let req1_hash = sha1_hash(&[b"req1", &s_bytes]);
    read_until(&mut stream, &req1_hash, 512 + 20).await?;

    // Read 20-byte hash
    let mut b_hash = [0u8; 20];
    stream.read_exact(&mut b_hash).await?;

    let req3_hash = sha1_hash(&[b"req3", &s_bytes]);

    // Find matching skey
    let mut matched_skey = None;
    for cand in skey_candidates {
        let req2_cand = sha1_hash(&[b"req2", &cand]);
        let mut xored = [0u8; 20];
        for i in 0..20 {
            xored[i] = req2_cand[i] ^ req3_hash[i];
        }
        if xored == b_hash {
            matched_skey = Some(cand);
            break;
        }
    }

    let skey = matched_skey
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "no matching secret key found"))?;

    let key_a = sha1_hash(&[b"keyA", &s_bytes, &skey]);
    let mut dec_a = Rc4::new(&key_a);

    // Read VC (8 bytes), crypto_provide (4 bytes), pad_c_len (2 bytes)
    let mut init_hdr = [0u8; 14];
    stream.read_exact(&mut init_hdr).await?;
    dec_a.apply(&mut init_hdr);

    if init_hdr[0..8] != [0u8; 8] {
        return Err(Error::new(ErrorKind::InvalidData, "invalid VC in MSE"));
    }

    let crypto_provides = CryptoMethod(u32::from_be_bytes([
        init_hdr[8],
        init_hdr[9],
        init_hdr[10],
        init_hdr[11],
    ]));
    let pad_c_len = u16::from_be_bytes([init_hdr[12], init_hdr[13]]) as usize;

    if pad_c_len > 0 {
        let mut pad_c = vec![0u8; pad_c_len];
        stream.read_exact(&mut pad_c).await?;
        dec_a.apply(&mut pad_c);
    }

    // Read len(IA)
    let mut ia_len_bytes = [0u8; 2];
    stream.read_exact(&mut ia_len_bytes).await?;
    dec_a.apply(&mut ia_len_bytes);
    let ia_len = u16::from_be_bytes(ia_len_bytes) as usize;

    let mut ia = vec![0u8; ia_len];
    if ia_len > 0 {
        stream.read_exact(&mut ia).await?;
        dec_a.apply(&mut ia);
    }

    // Select crypto method
    let chosen = selector(crypto_provides);

    // KeyB for writing to initiator
    let key_b = sha1_hash(&[b"keyB", &s_bytes, &skey]);
    let mut enc_b = Rc4::new(&key_b);

    let pad_d_len = {
        let mut rng = rand::thread_rng();
        rng.gen_range(0..=MAX_PAD_LEN) as u16
    };
    let pad_d = vec![0u8; pad_d_len as usize];

    let mut reply = Vec::with_capacity(8 + 4 + 2 + pad_d.len());
    reply.extend_from_slice(&[0u8; 8]); // VC
    reply.extend_from_slice(&chosen.0.to_be_bytes());
    reply.extend_from_slice(&pad_d_len.to_be_bytes());
    reply.extend_from_slice(&pad_d);

    enc_b.apply(&mut reply);
    stream.write_all(&reply).await?;
    stream.flush().await?;

    let (dec, enc) = if chosen == CryptoMethod::RC4 {
        (Some(dec_a), Some(enc_b))
    } else {
        (None, None)
    };

    Ok(ReceiverResult {
        stream: MseStream::new(stream, dec, enc, Bytes::from(ia)),
        crypto_method: chosen,
        secret_key: skey,
    })
}

/// Handle encryption for inbound stream based on `HeaderObfuscationPolicy`.
pub async fn handle_encryption<S: AsyncRead + AsyncWrite + Unpin, I, F>(
    mut stream: S,
    skeys: I,
    policy: HeaderObfuscationPolicy,
    selector: F,
) -> io::Result<(MseStream<S>, bool, CryptoMethod)>
where
    I: IntoIterator<Item = Vec<u8>> + Clone,
    F: Fn(CryptoMethod) -> CryptoMethod,
{
    if !policy.require_preferred || !policy.preferred {
        // Peek or test for plain BitTorrent handshake
        let mut proto = [0u8; 20];
        stream.read_exact(&mut proto).await?;
        if &proto == crate::handshake::PROTOCOL_HEADER {
            let res_stream = MseStream::new(stream, None, None, Bytes::copy_from_slice(&proto));
            return Ok((res_stream, false, CryptoMethod::PLAINTEXT));
        }

        if policy.require_preferred {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "unexpected protocol header and header obfuscation disabled",
            ));
        }

        // Put bytes back into stream wrapper and proceed with MSE
        let mut y_a = vec![0u8; 96];
        y_a[..20].copy_from_slice(&proto);
        stream.read_exact(&mut y_a[20..96]).await?;
        // Create an adapter that feeds the 96-byte Y_A to receive_handshake
        // We can run receive_handshake directly:
        let p = prime_p();
        let g = BigUint::from(2u32);
        let y_a_biguint = BigUint::from_bytes_be(&y_a);

        let (x_bytes, pad_b) = {
            let mut rng = rand::thread_rng();
            let mut x = [0u8; 20];
            rng.fill(&mut x[..]);
            let pad = random_pad(MAX_PAD_LEN);
            (x, pad)
        };
        let x_b = BigUint::from_bytes_be(&x_bytes);
        let y_b = g.modpow(&x_b, &p);
        let y_b_bytes = pad_to_96(&y_b);

        stream.write_all(&y_b_bytes).await?;
        stream.write_all(&pad_b).await?;
        stream.flush().await?;

        let s = y_a_biguint.modpow(&x_b, &p);
        let s_bytes = pad_to_96(&s);

        let req1_hash = sha1_hash(&[b"req1", &s_bytes]);
        read_until(&mut stream, &req1_hash, 512 + 20).await?;

        let mut b_hash = [0u8; 20];
        stream.read_exact(&mut b_hash).await?;

        let req3_hash = sha1_hash(&[b"req3", &s_bytes]);

        let mut matched = None;
        for cand in skeys {
            let req2 = sha1_hash(&[b"req2", &cand]);
            let mut xored = [0u8; 20];
            for i in 0..20 {
                xored[i] = req2[i] ^ req3_hash[i];
            }
            if xored == b_hash {
                matched = Some(cand);
                break;
            }
        }

        let skey = matched.ok_or_else(|| Error::new(ErrorKind::NotFound, "no skey matched"))?;
        let key_a = sha1_hash(&[b"keyA", &s_bytes, &skey]);
        let mut dec_a = Rc4::new(&key_a);

        let mut init_hdr = [0u8; 14];
        stream.read_exact(&mut init_hdr).await?;
        dec_a.apply(&mut init_hdr);

        if init_hdr[0..8] != [0u8; 8] {
            return Err(Error::new(ErrorKind::InvalidData, "invalid VC"));
        }

        let crypto_provides = CryptoMethod(u32::from_be_bytes([
            init_hdr[8],
            init_hdr[9],
            init_hdr[10],
            init_hdr[11],
        ]));
        let pad_c_len = u16::from_be_bytes([init_hdr[12], init_hdr[13]]) as usize;

        if pad_c_len > 0 {
            let mut pad_c = vec![0u8; pad_c_len];
            stream.read_exact(&mut pad_c).await?;
            dec_a.apply(&mut pad_c);
        }

        let mut ia_len_b = [0u8; 2];
        stream.read_exact(&mut ia_len_b).await?;
        dec_a.apply(&mut ia_len_b);
        let ia_len = u16::from_be_bytes(ia_len_b) as usize;

        let mut ia = vec![0u8; ia_len];
        if ia_len > 0 {
            stream.read_exact(&mut ia).await?;
            dec_a.apply(&mut ia);
        }

        let chosen = selector(crypto_provides);

        let key_b = sha1_hash(&[b"keyB", &s_bytes, &skey]);
        let mut enc_b = Rc4::new(&key_b);

        let pad_d_len = {
            let mut rng = rand::thread_rng();
            rng.gen_range(0..=MAX_PAD_LEN) as u16
        };
        let pad_d = vec![0u8; pad_d_len as usize];

        let mut reply = Vec::with_capacity(8 + 4 + 2 + pad_d.len());
        reply.extend_from_slice(&[0u8; 8]);
        reply.extend_from_slice(&chosen.0.to_be_bytes());
        reply.extend_from_slice(&pad_d_len.to_be_bytes());
        reply.extend_from_slice(&pad_d);

        enc_b.apply(&mut reply);
        stream.write_all(&reply).await?;
        stream.flush().await?;

        let (dec, enc) = if chosen == CryptoMethod::RC4 {
            (Some(dec_a), Some(enc_b))
        } else {
            (None, None)
        };

        return Ok((
            MseStream::new(stream, dec, enc, Bytes::from(ia)),
            true,
            chosen,
        ));
    }

    // Require obfuscation
    let res = receive_handshake(stream, skeys, selector).await?;
    Ok((res.stream, true, res.crypto_method))
}
