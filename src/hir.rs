#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type {
    Int,
    Bool,
    String,
    Unit,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub id: usize,
    pub name: String,
    pub params: Vec<Local>,
    pub result: Type,
    pub locals: Vec<Local>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub struct Local {
    pub id: usize,
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
}

pub type Block = Vec<Stmt>;

#[derive(Clone, Debug)]
pub enum Stmt {
    Let {
        local: usize,
        value: Expr,
    },
    Assign {
        local: usize,
        op: AssignOp,
        value: Expr,
    },
    Return(Option<Expr>),
    Expr(Expr),
    If {
        condition: Expr,
        then_block: Block,
        else_block: Block,
    },
    For {
        local: usize,
        start: Expr,
        end: Expr,
        body: Block,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Bool(bool),
    String(String),
    Local(usize),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        function: usize,
        name: String,
        args: Vec<Expr>,
    },
    Print(Vec<Expr>),
    /// Division lowered to raw `sdiv` after a proof that the divisor cannot be
    /// zero and cannot be the `INT64_MIN / -1` overflow case.
    RawDiv {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Remainder lowered to raw `srem` under the same proof as [`ExprKind::RawDiv`].
    RawRem {
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}
