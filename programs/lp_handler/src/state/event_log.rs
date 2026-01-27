use anchor_lang::prelude::borsh::maybestd::io;
use anchor_lang::prelude::borsh::maybestd::io::Write;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::log::sol_log_data;

use crate::LpDepositError;

/// 一个简单的固定缓冲区 writer，用于把 borsh 序列化写入栈上 buffer，避免堆分配。
struct FixedBuf<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> FixedBuf<'a> {
    #[inline(always)]
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    #[inline(always)]
    fn as_written(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
}

impl<'a> io::Write for FixedBuf<'a> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remain = self.buf.len().saturating_sub(self.pos);
        if bytes.len() > remain {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "fixed buffer overflow",
            ));
        }
        let end = self.pos + bytes.len();
        self.buf[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 无堆分配写事件（Anchor 兼容）：`DISCRIMINATOR + borsh(event)`，然后 `sol_log_data` 输出。
///
/// 注意：为了避免增大调用者栈帧，把 buffer 放在本函数里，并标注 `#[inline(never)]`。
#[inline(never)]
pub fn log_event_no_heap<E>(event: &E) -> Result<()>
where
    E: anchor_lang::Event + anchor_lang::prelude::borsh::BorshSerialize,
{
    // 事件通常 < 300 bytes；留 512 足够且不会显著影响栈。
    let mut buf = [0u8; 512];
    let mut w = FixedBuf::new(&mut buf);

    w.write_all(&E::DISCRIMINATOR)
        .map_err(|_| error!(LpDepositError::EventLogSerializeFailed))?;
    event
        .serialize(&mut w)
        .map_err(|_| error!(LpDepositError::EventLogSerializeFailed))?;

    sol_log_data(&[w.as_written()]);
    Ok(())
}
