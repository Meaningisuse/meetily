use crate::meeting::EnhancementError;

/// Byte-fallback tokenizers cannot emit more source tokens than UTF-8 bytes.
/// Without the exact model tokenizer, this intentionally conservative bound
/// avoids assuming Chinese has the English chars/token ratio.
pub struct TokenBudget;
#[derive(Debug, Clone)]
pub struct SummaryChunk {
    pub segments: Vec<String>,
    pub estimated_tokens: usize,
}
impl SummaryChunk {
    pub fn text(&self) -> String {
        self.segments.concat()
    }
}
impl TokenBudget {
    pub fn estimate(text: &str) -> usize {
        text.len()
    }
    pub fn available(
        context: usize,
        prompt: usize,
        output: usize,
    ) -> Result<usize, EnhancementError> {
        context
            .checked_sub(prompt.saturating_add(output).saturating_add(64))
            .filter(|n| *n >= 4)
            .ok_or_else(|| {
                EnhancementError::invalid(
                    "Prompt and output budgets leave no transcript capacity in this model context.",
                )
            })
    }
    pub fn split(
        segments: &[&str],
        context: usize,
        prompt: usize,
        output: usize,
    ) -> Result<Vec<SummaryChunk>, EnhancementError> {
        let capacity = Self::available(context, prompt, output)?;
        let mut chunks = Vec::new();
        let mut current = SummaryChunk {
            segments: Vec::new(),
            estimated_tokens: 0,
        };
        for segment in segments {
            let mut remaining = *segment;
            while !remaining.is_empty() {
                let left = capacity - current.estimated_tokens;
                let mut end = remaining.len().min(left);
                while end > 0 && !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                if end == 0 {
                    if current.segments.is_empty() {
                        return Err(EnhancementError::invalid(
                            "A Unicode character exceeds the available budget.",
                        ));
                    }
                    chunks.push(current);
                    current = SummaryChunk {
                        segments: Vec::new(),
                        estimated_tokens: 0,
                    };
                    continue;
                }
                current.segments.push(remaining[..end].to_owned());
                current.estimated_tokens += end;
                remaining = &remaining[end..];
                if current.estimated_tokens == capacity {
                    chunks.push(current);
                    current = SummaryChunk {
                        segments: Vec::new(),
                        estimated_tokens: 0,
                    };
                }
            }
        }
        if !current.segments.is_empty() {
            chunks.push(current);
        }
        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::TokenBudget;
    #[test]
    fn chinese_budget_and_all_providers_chunk_consistently() {
        let transcript = format!(
            "{}\n最后行动项：周五完成核对。",
            "中文会议 mixed English 内容。\n".repeat(100)
        );
        for (context, prompt, output) in
            [(4096, 1000, 512), (8192, 1400, 2048), (32768, 3000, 4096)]
        {
            let segments: Vec<&str> = transcript.split_inclusive('\n').collect();
            let chunks = TokenBudget::split(&segments, context, prompt, output).unwrap();
            assert!(!chunks.is_empty());
            for chunk in &chunks {
                assert!(chunk.estimated_tokens + prompt + output + 64 <= context);
            }
            let restored: String = chunks
                .iter()
                .flat_map(|c| c.segments.iter())
                .map(String::as_str)
                .collect();
            assert_eq!(restored, transcript);
            assert!(restored.ends_with("周五完成核对。"));
        }
        assert!(TokenBudget::estimate("中文预算") >= 4);
        assert!(TokenBudget::split(&["正文"], 100, 80, 40).is_err());
    }
    #[test]
    fn oversized_single_segment_keeps_every_unicode_character() {
        let original = "中文🙂 English 🚀".repeat(200);
        let chunks = TokenBudget::split(&[&original], 512, 100, 128).unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|c| c.segments.concat())
                .collect::<String>(),
            original
        );
        assert!(chunks.iter().all(|c| c.estimated_tokens <= 220));
    }
}
