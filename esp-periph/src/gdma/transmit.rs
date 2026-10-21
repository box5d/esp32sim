//! Finite GDMA OUT gathers. Descriptors are read only through the shared
//! `DescriptorWalk`; this file adds the OUT-side bookkeeping, not a second reader.
use super::{DescriptorWalk, DmaDescriptorFault, DmaDescriptorWord, GdmaOutCh, GDMA_DESCRIPTOR_STEP_BUDGET};
use emu_core::Fault;

/// Memory behind an OUT gather: descriptor words, buffer bytes and DW0 write-back.
/// DMA accesses are unpriced and are not CPU cache traffic. The channel is reached through
/// the memory so that a write-back landing on a GDMA register keeps its side effects.
pub trait OutMemory {
    fn out_channel(&mut self) -> &mut GdmaOutCh;
    fn word(&mut self, address: u32) -> Result<u32, Fault>;
    /// Append `count` bytes, or report the first address that cannot be read.
    fn append(&mut self, address: u32, count: usize, out: &mut Vec<u8>) -> Result<(), (u32, Fault)>;
    fn writeback(&mut self, address: u32, value: u32) -> Result<(), Fault>;
}

/// C3/C6 crypto DMA reaches data SRAM only, never MMIO. `base` is the bus address of `bytes[0]`.
pub struct SramOut<'a> { pub base: u32, pub bytes: &'a mut [u8], pub channel: &'a mut GdmaOutCh }
impl SramOut<'_> {
    fn range(&self, address: u32, count: usize) -> Result<std::ops::Range<usize>, Fault> {
        let start = address.checked_sub(self.base).ok_or(Fault::Unmapped)? as usize;
        let end = start.checked_add(count).filter(|&n| n <= self.bytes.len()).ok_or(Fault::Unmapped)?;
        Ok(start..end)
    }
}
impl OutMemory for SramOut<'_> {
    fn out_channel(&mut self) -> &mut GdmaOutCh { self.channel }
    fn word(&mut self, address: u32) -> Result<u32, Fault> {
        let range = self.range(address, 4)?;
        Ok(u32::from_le_bytes(self.bytes[range].try_into().unwrap()))
    }
    fn append(&mut self, address: u32, count: usize, out: &mut Vec<u8>) -> Result<(), (u32, Fault)> {
        let range = self.range(address, count).map_err(|fault| (address, fault))?;
        out.extend_from_slice(&self.bytes[range]);
        Ok(())
    }
    fn writeback(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        let range = self.range(address, 4)?;
        self.bytes[range].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
}

/// Gather at most `limit` bytes from the channel's OUT chain into `out`, stopping after the
/// first SUC_EOF descriptor or at a null link. Returns the link after the last descriptor read.
/// On a fault `out` keeps what was gathered before it; the caller decides how to complete.
/// One `DescriptorWalk` bounds the chain to `GDMA_DESCRIPTOR_STEP_BUDGET` descriptors
/// (4096 * 4095 bytes); a visited set rejects cycles independently of owner checking.
/// IDF v5.5.5 components/soc/{esp32s3,esp32c3,esp32c6}/register/soc/gdma_reg.h:
/// OUT_CONF1 CHECK_OWNER bit 12; OUT_DONE/OUT_EOF are bits 0/1 of the OUT interrupt block.
/// C3's register adapter translates them to its combined IN/OUT layout.
pub fn gather_out(memory: &mut impl OutMemory, limit: usize, out: &mut Vec<u8>) -> Result<u32, DmaDescriptorFault> {
    let mut desc = memory.out_channel().desc;
    let mut visited = std::collections::HashSet::new();
    let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
    let start = out.len();
    while desc != 0 && out.len() - start < limit {
        if !visited.insert(desc) { return Err(DmaDescriptorFault::Cycle { descriptor: desc }); }
        // Descriptors are word structures (hal/include/hal/dma_types.h:23-32).
        if desc & 3 != 0 { return Err(DmaDescriptorFault::Read { descriptor: desc, word: DmaDescriptorWord::Control, fault: Fault::Misaligned }); }
        let (control, d) = walk.read_with(desc, |address| memory.word(address))?;
        // LENGTH counts valid bytes of a SIZE-byte buffer; more is a malformed descriptor.
        if d.length > d.size { return Err(DmaDescriptorFault::BufferRead { descriptor: desc, address: d.buf, fault: Fault::Prohibited }); }
        if memory.out_channel().conf1 & (1 << 12) != 0 && !d.owner_dma { return Err(DmaDescriptorFault::NotOwned { descriptor: desc }); }
        let take = (d.length as usize).min(limit - (out.len() - start));
        memory.append(d.buf, take, out).map_err(|(address, fault)| DmaDescriptorFault::BufferRead { descriptor: desc, address, fault })?;
        // OUT_AUTO_WRBACK (OUT_CONF0 bit 2) returns the descriptor to the CPU; without it the
        // owner bit stays as written. IDF v5.5.4 soc/esp32s3/register/soc/gdma_reg.h:585-591,
        // the C3/C6 headers at the same field. IDF v5.5.5's RMT TX driver sets it
        // (esp_driver_rmt/src/rmt_tx.c:42-46); its crypto GDMA applies no strategy and PARLIO
        // clears it (esp_driver_parlio/src/parlio_tx.c:169-173), so those keep the owner bit.
        if memory.out_channel().conf0 & (1 << 2) != 0 {
            memory.writeback(desc, control & !(1 << 31)).map_err(|fault| DmaDescriptorFault::Writeback { descriptor: desc, fault })?;
        }
        memory.out_channel().int_raw |= 1;
        if d.eof {
            memory.out_channel().int_raw |= 1 << 1;
            memory.out_channel().eof_desc = desc;
            return Ok(d.next);
        }
        desc = d.next;
    }
    Ok(desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(count: usize, eof_at: usize) -> Vec<u8> {
        let mut bytes = vec![0; count * 12];
        for i in 0..count {
            let offset = i * 12;
            let control = (1u32 << 31) | if i == eof_at { 1 << 30 } else { 0 };
            bytes[offset..offset + 4].copy_from_slice(&control.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&0x1000u32.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&(0x1000u32 + offset as u32 + 12).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn zero_progress_chain_stops_at_the_shared_descriptor_budget() {
        // 4096 empty descriptors and an EOF one: the 4097th read exceeds the walker's budget.
        let mut bytes = chain(GDMA_DESCRIPTOR_STEP_BUDGET + 1, GDMA_DESCRIPTOR_STEP_BUDGET);
        let mut channel = GdmaOutCh { desc: 0x1000, ..Default::default() };
        let mut memory = SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        let fault = gather_out(&mut memory, 64, &mut Vec::new()).unwrap_err();
        assert_eq!(fault, DmaDescriptorFault::StepBudgetExceeded { budget: GDMA_DESCRIPTOR_STEP_BUDGET });
        // One fewer descriptor fits the budget and ends at its EOF.
        let mut bytes = chain(GDMA_DESCRIPTOR_STEP_BUDGET, GDMA_DESCRIPTOR_STEP_BUDGET - 1);
        let mut channel = GdmaOutCh { desc: 0x1000, ..Default::default() };
        let mut memory = SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        assert!(gather_out(&mut memory, 64, &mut Vec::new()).is_ok());
        assert_eq!(channel.int_raw, 3);
    }

    #[test]
    fn sram_bounds_owner_and_partial_output() {
        // Descriptor 0 holds 4 bytes and links to descriptor 1, whose buffer leaves SRAM.
        let mut bytes = vec![0u8; 64];
        let words = [(1u32 << 31) | (4 << 12) | 4, 0x1030, 0x100c, (1 << 31) | (1 << 30) | (4 << 12) | 4, 0x1040, 0];
        for (i, w) in words.iter().enumerate() { bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes()); }
        bytes[0x30..0x34].copy_from_slice(&[1, 2, 3, 4]);
        let mut channel = GdmaOutCh { desc: 0x1000, conf0: 1 << 2, conf1: 1 << 12, ..Default::default() };
        let mut memory = SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        let mut out = Vec::new();
        let fault = gather_out(&mut memory, usize::MAX, &mut out).unwrap_err();
        assert_eq!(fault, DmaDescriptorFault::BufferRead { descriptor: 0x100c, address: 0x1040, fault: Fault::Unmapped });
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()) >> 31, 0, "first descriptor returned to the CPU");
        // With CHECK_OWNER, the now CPU-owned descriptor is rejected before its buffer is read.
        let mut channel = GdmaOutCh { desc: 0x1000, conf1: 1 << 12, ..Default::default() };
        let mut memory = SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        assert_eq!(gather_out(&mut memory, usize::MAX, &mut Vec::new()), Err(DmaDescriptorFault::NotOwned { descriptor: 0x1000 }));
        // A descriptor that straddles the end of SRAM faults on the word that leaves it.
        let mut channel = GdmaOutCh { desc: 0x1000 + 56, ..Default::default() };
        let mut memory = SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        assert!(matches!(gather_out(&mut memory, 4, &mut Vec::new()), Err(DmaDescriptorFault::Read { descriptor: 0x1038, .. })));
    }

    #[test]
    fn writeback_gate_alignment_and_length_rules() {
        let words = [(1u32 << 31) | (1 << 30) | (4 << 12) | 4, 0x1030, 0];
        let image = |control: u32| {
            let mut bytes = vec![0u8; 64];
            for (i, w) in [control, words[1], words[2]].iter().enumerate() { bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes()); }
            bytes
        };
        // Without OUT_AUTO_WRBACK the owner bit is left as written.
        let mut bytes = image(words[0]);
        let mut channel = GdmaOutCh { desc: 0x1000, conf1: 1 << 12, ..Default::default() };
        assert_eq!(gather_out(&mut SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel }, 64, &mut Vec::new()), Ok(0));
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), words[0]);
        // A misaligned descriptor address and LENGTH > SIZE are descriptor errors.
        let mut channel = GdmaOutCh { desc: 0x1002, ..Default::default() };
        assert!(matches!(gather_out(&mut SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel }, 64, &mut Vec::new()),
            Err(DmaDescriptorFault::Read { descriptor: 0x1002, fault: Fault::Misaligned, .. })));
        let mut bytes = image((1 << 31) | (1 << 30) | (5 << 12) | 4);
        let mut channel = GdmaOutCh { desc: 0x1000, ..Default::default() };
        let mut out = Vec::new();
        assert!(matches!(gather_out(&mut SramOut { base: 0x1000, bytes: &mut bytes, channel: &mut channel }, 64, &mut out),
            Err(DmaDescriptorFault::BufferRead { descriptor: 0x1000, .. })));
        assert!(out.is_empty());
    }
}
