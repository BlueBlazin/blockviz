//! A GPT language model, as in nanoGPT.
use candle_core::{Module, Result, Tensor, D};
use candle_nn::{embedding, layer_norm, linear, Embedding, LayerNorm, Linear, VarBuilder};

pub struct Config {
    pub vocab_size: usize,
    pub n_layer: usize,
    pub n_head: usize,
    pub n_embd: usize,
}

struct CausalSelfAttention {
    c_attn: Linear,
    c_proj: Linear,
    n_head: usize,
}

impl CausalSelfAttention {
    fn new(cfg: &Config, vb: VarBuilder) -> Result<Self> {
        let c_attn = linear(cfg.n_embd, 3 * cfg.n_embd, vb.pp("c_attn"))?;
        let c_proj = linear(cfg.n_embd, cfg.n_embd, vb.pp("c_proj"))?;
        Ok(Self { c_attn, c_proj, n_head: cfg.n_head })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, t, c) = x.dims3()?;
        let qkv = self.c_attn.forward(x)?;
        let q = qkv.narrow(2, 0, c)?;
        let k = qkv.narrow(2, c, c)?;
        let v = qkv.narrow(2, 2 * c, c)?;
        let att = (q.matmul(&k.t()?)? / (c as f64).sqrt())?;
        let att = candle_nn::ops::softmax(&att, D::Minus1)?;
        let y = att.matmul(&v)?.reshape((b, t, c))?;
        self.c_proj.forward(&y)
    }
}

struct Mlp {
    c_fc: Linear,
    c_proj: Linear,
}

impl Mlp {
    fn new(cfg: &Config, vb: VarBuilder) -> Result<Self> {
        Ok(Self { c_fc: linear(cfg.n_embd, 4 * cfg.n_embd, vb.pp("c_fc"))?, c_proj: linear(4 * cfg.n_embd, cfg.n_embd, vb.pp("c_proj"))? })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = self.c_fc.forward(x)?.gelu()?;
        self.c_proj.forward(&x)
    }
}

struct Block {
    ln_1: LayerNorm,
    attn: CausalSelfAttention,
    ln_2: LayerNorm,
    mlp: Mlp,
}

impl Block {
    fn new(cfg: &Config, vb: VarBuilder) -> Result<Self> {
        let ln_1 = layer_norm(cfg.n_embd, 1e-5, vb.pp("ln_1"))?;
        let attn = CausalSelfAttention::new(cfg, vb.pp("attn"))?;
        let ln_2 = layer_norm(cfg.n_embd, 1e-5, vb.pp("ln_2"))?;
        let mlp = Mlp::new(cfg, vb.pp("mlp"))?;
        Ok(Self { ln_1, attn, ln_2, mlp })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = (x + self.attn.forward(&self.ln_1.forward(x)?)?)?;
        let x = (&x + self.mlp.forward(&self.ln_2.forward(&x)?)?)?;
        Ok(x)
    }
}

pub struct Gpt {
    wte: Embedding,
    wpe: Embedding,
    blocks: Vec<Block>,
    ln_f: LayerNorm,
    lm_head: Linear,
}

impl Gpt {
    pub fn new(cfg: &Config, vb: VarBuilder) -> Result<Self> {
        let wte = embedding(cfg.vocab_size, cfg.n_embd, vb.pp("wte"))?;
        let wpe = embedding(1024, cfg.n_embd, vb.pp("wpe"))?;
        let blocks = (0..cfg.n_layer).map(|i| Block::new(cfg, vb.pp(format!("h.{i}")))).collect::<Result<Vec<_>>>()?;
        let ln_f = layer_norm(cfg.n_embd, 1e-5, vb.pp("ln_f"))?;
        let lm_head = linear(cfg.n_embd, cfg.vocab_size, vb.pp("lm_head"))?;
        Ok(Self { wte, wpe, blocks, ln_f, lm_head })
    }

    pub fn forward(&self, idx: &Tensor) -> Result<Tensor> {
        let (_b, t) = idx.dims2()?;
        let pos = Tensor::arange(0u32, t as u32, idx.device())?;
        let tok_emb = self.wte.forward(idx)?;
        let pos_emb = self.wpe.forward(&pos)?;
        let mut x = tok_emb.broadcast_add(&pos_emb)?;
        for block in self.blocks.iter() {
            x = block.forward(&x)?;
        }
        let x = self.ln_f.forward(&x)?;
        self.lm_head.forward(&x)
    }
}
