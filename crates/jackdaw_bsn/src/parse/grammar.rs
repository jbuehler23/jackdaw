// auto-generated: "lalrpop 0.23.1"
// sha3: 0c177b78ca4644185e7bd0329f6341309f4a7c38d320f213b7318ab8a349a8bc
use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
use crate::parse::lexer;
use bevy::ecs::entity::Entity;
use std::cell::RefCell;
#[allow(unused_extern_crates)]
extern crate lalrpop_util as __lalrpop_util;
#[allow(unused_imports)]
use self::__lalrpop_util::state_machine as __state_machine;
#[allow(unused_extern_crates)]
extern crate alloc;

#[rustfmt::skip]
#[allow(explicit_outlives_requirements, non_snake_case, non_camel_case_types, unused_mut, unused_variables, unused_imports, unused_parens, clippy::needless_lifetimes, clippy::type_complexity, clippy::needless_return, clippy::too_many_arguments, clippy::match_single_binding, clippy::clone_on_copy, clippy::unit_arg)]
mod __parse__Patch {

    use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
    use crate::parse::lexer;
    use bevy::ecs::entity::Entity;
    use std::cell::RefCell;
    #[allow(unused_extern_crates)]
    extern crate lalrpop_util as __lalrpop_util;
    #[allow(unused_imports)]
    use self::__lalrpop_util::state_machine as __state_machine;
    #[allow(unused_extern_crates)]
    extern crate alloc;
    use super::__ToTriple;
    #[allow(dead_code)]
    pub(crate) enum __Symbol<>
     {
        Variant0(String),
        Variant1(f64),
        Variant2(bool),
        Variant3(i128),
        Variant4(lexer::Token),
        Variant5(Option<lexer::Token>),
        Variant6(BsnValue),
        Variant7(BsnField),
        Variant8(Vec<BsnField>),
        Variant9((BsnValue, BsnValue)),
        Variant10(Vec<(BsnValue, BsnValue)>),
        Variant11(Vec<BsnValue>),
        Variant12(Entity),
        Variant13(alloc::vec::Vec<Entity>),
        Variant14(Vec<Entity>),
    }
    const __ACTION: &[i8] = &[
        // State 0
        34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 33, 32, 2, 0, 0,
        // State 1
        34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 2
        34, 48, 45, 44, 46, 8, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 3
        34, 0, 0, 0, 0, 0, -51, 0, 0, 0, 0, 0, 0, 0, 33, 32, 2, 0, 0,
        // State 4
        56, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 5
        34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 6
        34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 7
        34, 48, 45, 44, 46, 8, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 8
        34, 0, 0, 0, 0, 0, -49, 0, 0, 0, 0, -49, 0, 0, 33, 32, 2, 0, 0,
        // State 9
        34, 48, 45, 44, 46, 8, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 10
        56, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 11
        34, 48, 45, 44, 46, 8, 0, 0, -34, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 12
        34, 48, 45, 44, 46, 8, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 13
        56, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 14
        0, 0, 0, 0, 0, 0, 76, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 15
        34, 0, 0, 0, 0, 0, -52, 0, 0, 0, 0, 0, 0, 0, 33, 32, 2, 0, 0,
        // State 16
        56, 0, 0, 0, 0, 0, 0, 0, 0, 0, -24, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 17
        34, 48, 45, 44, 46, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 18
        34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 19
        34, 48, 45, 44, 46, 8, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 20
        56, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 21
        34, 48, 45, 44, 46, 8, 87, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 22
        34, 48, 45, 44, 46, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 23
        0, 0, 0, 0, 0, 0, 93, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 24
        34, 48, 45, 44, 46, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 47,
        // State 25
        -39, 0, 0, 0, 0, 0, -39, 0, 0, 0, 0, -39, 0, 0, -39, -39, -39, 0, 0,
        // State 26
        -38, 0, 0, 0, 0, 0, -38, 0, 0, 0, 0, -38, 0, 0, -38, -38, -38, 0, 0,
        // State 27
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 28
        -40, 0, 0, 0, 0, 0, -40, 0, 0, 0, 0, -40, 0, 0, -40, -40, -40, 0, 0,
        // State 29
        -42, 0, 0, 0, 0, 4, -42, 3, 0, 5, 0, -42, 0, 0, -42, -42, -42, 0, 0,
        // State 30
        -56, 0, 0, 0, 0, -56, -56, -56, -56, -56, -56, -56, 6, -56, -56, -56, -56, 35, 0,
        // State 31
        36, 37, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 32
        0, 38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 33
        -61, 0, 0, 0, 0, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, 0,
        // State 34
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 35
        -31, 0, 0, 0, 0, 0, -31, 0, 0, 0, 0, -31, 0, 0, -31, -31, -31, 0, 0,
        // State 36
        -32, 0, 0, 0, 0, 0, -32, 0, 0, 0, 0, -32, 0, 0, -32, -32, -32, 0, 0,
        // State 37
        -5, 0, 0, 0, 0, 0, -5, 0, 0, 0, 0, -5, 0, 0, -5, -5, -5, 0, 0,
        // State 38
        -41, 0, 0, 0, 0, 0, -41, 10, 0, 11, 0, -41, 0, 0, -41, -41, -41, 0, 0,
        // State 39
        0, 0, 0, 0, 0, 0, -36, 0, -36, 0, 0, -36, 0, 0, 0, 0, 0, 0, 0,
        // State 40
        0, 0, 0, 0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 41
        0, 0, 0, 0, 0, 0, 0, 0, -35, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0,
        // State 42
        0, 0, 0, 0, 0, 0, -7, 13, -7, 14, -7, -7, 0, 0, 0, 0, 0, 0, 0,
        // State 43
        0, 0, 0, 0, 0, 0, -14, 0, -14, 0, -14, -14, 0, 0, 0, 0, 0, 0, 0,
        // State 44
        0, 0, 0, 0, 0, 0, -13, 0, -13, 0, -13, -13, 0, 0, 0, 0, 0, 0, 0,
        // State 45
        0, 0, 0, 0, 0, 0, -15, 0, -15, 0, -15, -15, 0, 0, 0, 0, 0, 0, 0,
        // State 46
        0, 0, 0, 0, 0, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 47
        0, 0, 0, 0, 0, 0, -12, 0, -12, 0, -12, -12, 0, 0, 0, 0, 0, 0, 0,
        // State 48
        -47, 0, 0, 0, 0, 0, -47, 0, 0, 0, 0, -47, 0, 0, -47, -47, -47, 0, 0,
        // State 49
        0, 0, 0, 0, 0, 0, -55, 0, 0, 0, 0, -55, 0, 0, 0, 0, 0, 0, 0,
        // State 50
        0, 0, 0, 0, 0, 0, 65, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 51
        0, 0, 0, 0, 0, 0, -53, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0,
        // State 52
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -26, -26, 0, 0, 0, 0, 0, 0, 0,
        // State 53
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 66, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 54
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -25, 17, 0, 0, 0, 0, 0, 0, 0,
        // State 55
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 18, 0, 0, 0, 0,
        // State 56
        -60, 0, 0, 0, 0, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, 0,
        // State 57
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -58, 0, -58, 0, 0, 0, 0, 0,
        // State 58
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 19, 0, 67, 0, 0, 0, 0, 0,
        // State 59
        -46, 0, 0, 0, 0, 0, -46, 0, 0, 0, 0, -46, 0, 0, -46, -46, -46, 0, 0,
        // State 60
        0, 0, 0, 0, 0, 0, -6, 20, -6, 21, -6, -6, 0, 0, 0, 0, 0, 0, 0,
        // State 61
        0, 0, 0, 0, 0, 0, 73, 0, 0, 0, 0, 22, 0, 0, 0, 0, 0, 0, 0,
        // State 62
        0, 0, 0, 0, 0, 0, -18, 0, -18, 0, -18, -18, 0, 0, 0, 0, 0, 0, 0,
        // State 63
        -48, 0, 0, 0, 0, 0, -48, 0, 0, 0, 0, -48, 0, 0, -48, -48, -48, 0, 0,
        // State 64
        -50, 0, 0, 0, 0, 0, -50, 0, 0, 0, 0, -50, 0, 0, -50, -50, -50, 0, 0,
        // State 65
        -44, 0, 0, 0, 0, 0, -44, 0, 0, 0, 0, -44, 0, 0, -44, -44, -44, 0, 0,
        // State 66
        -57, 0, 0, 0, 0, -57, -57, -57, -57, -57, -57, -57, 0, -57, -57, -57, -57, 0, 0,
        // State 67
        0, 0, 0, 0, 0, 0, 0, 0, 81, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 68
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 82, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 69
        0, 0, 0, 0, 0, 0, -37, 0, -37, 0, 0, -37, 0, 0, 0, 0, 0, 0, 0,
        // State 70
        0, 0, 0, 0, 0, 0, 0, 0, 83, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 71
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 84, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 72
        0, 0, 0, 0, 0, 0, -17, 0, -17, 0, -17, -17, 0, 0, 0, 0, 0, 0, 0,
        // State 73
        0, 0, 0, 0, 0, 0, -29, 0, 0, 0, 0, -29, 0, 0, 0, 0, 0, 0, 0,
        // State 74
        0, 0, 0, 0, 0, 0, 88, 0, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0,
        // State 75
        0, 0, 0, 0, 0, 0, -21, 0, -21, 0, -21, -21, 0, 0, 0, 0, 0, 0, 0,
        // State 76
        0, 0, 0, 0, 0, 0, -54, 0, 0, 0, 0, -54, 0, 0, 0, 0, 0, 0, 0,
        // State 77
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -27, -27, 0, 0, 0, 0, 0, 0, 0,
        // State 78
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -22, -22, 0, 0, 0, 0, 0, 0, 0,
        // State 79
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -59, 0, -59, 0, 0, 0, 0, 0,
        // State 80
        -45, 0, 0, 0, 0, 0, -45, 0, 0, 0, 0, -45, 0, 0, -45, -45, -45, 0, 0,
        // State 81
        -43, 0, 0, 0, 0, 0, -43, 0, 0, 0, 0, -43, 0, 0, -43, -43, -43, 0, 0,
        // State 82
        0, 0, 0, 0, 0, 0, -11, 0, -11, 0, -11, -11, 0, 0, 0, 0, 0, 0, 0,
        // State 83
        0, 0, 0, 0, 0, 0, -9, 0, -9, 0, -9, -9, 0, 0, 0, 0, 0, 0, 0,
        // State 84
        0, 0, 0, 0, 0, 0, 0, 0, 90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 85
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 91, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 86
        0, 0, 0, 0, 0, 0, -16, 0, -16, 0, -16, -16, 0, 0, 0, 0, 0, 0, 0,
        // State 87
        0, 0, 0, 0, 0, 0, -20, 0, -20, 0, -20, -20, 0, 0, 0, 0, 0, 0, 0,
        // State 88
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 25, 0, 0, 0, 0, 0, 0, 0,
        // State 89
        0, 0, 0, 0, 0, 0, -10, 0, -10, 0, -10, -10, 0, 0, 0, 0, 0, 0, 0,
        // State 90
        0, 0, 0, 0, 0, 0, -8, 0, -8, 0, -8, -8, 0, 0, 0, 0, 0, 0, 0,
        // State 91
        0, 0, 0, 0, 0, 0, -30, 0, 0, 0, 0, -30, 0, 0, 0, 0, 0, 0, 0,
        // State 92
        0, 0, 0, 0, 0, 0, -19, 0, -19, 0, -19, -19, 0, 0, 0, 0, 0, 0, 0,
        // State 93
        0, 0, 0, 0, 0, 0, 0, 0, 95, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 94
        0, 0, 0, 0, 0, 0, -28, 0, 0, 0, 0, -28, 0, 0, 0, 0, 0, 0, 0,
    ];
    fn __action(state: i8, integer: usize) -> i8 {
        __ACTION[(state as usize) * 19 + integer]
    }
    const __EOF_ACTION: &[i8] = &[
        // State 0
        0,
        // State 1
        0,
        // State 2
        0,
        // State 3
        0,
        // State 4
        0,
        // State 5
        0,
        // State 6
        0,
        // State 7
        0,
        // State 8
        0,
        // State 9
        0,
        // State 10
        0,
        // State 11
        0,
        // State 12
        0,
        // State 13
        0,
        // State 14
        0,
        // State 15
        0,
        // State 16
        0,
        // State 17
        0,
        // State 18
        0,
        // State 19
        0,
        // State 20
        0,
        // State 21
        0,
        // State 22
        0,
        // State 23
        0,
        // State 24
        0,
        // State 25
        -39,
        // State 26
        -38,
        // State 27
        -67,
        // State 28
        -40,
        // State 29
        -42,
        // State 30
        -56,
        // State 31
        0,
        // State 32
        0,
        // State 33
        -61,
        // State 34
        0,
        // State 35
        -31,
        // State 36
        -32,
        // State 37
        -5,
        // State 38
        -41,
        // State 39
        0,
        // State 40
        0,
        // State 41
        0,
        // State 42
        0,
        // State 43
        0,
        // State 44
        0,
        // State 45
        0,
        // State 46
        0,
        // State 47
        0,
        // State 48
        0,
        // State 49
        0,
        // State 50
        0,
        // State 51
        0,
        // State 52
        0,
        // State 53
        0,
        // State 54
        0,
        // State 55
        0,
        // State 56
        -60,
        // State 57
        0,
        // State 58
        0,
        // State 59
        -46,
        // State 60
        0,
        // State 61
        0,
        // State 62
        0,
        // State 63
        0,
        // State 64
        -50,
        // State 65
        -44,
        // State 66
        -57,
        // State 67
        0,
        // State 68
        0,
        // State 69
        0,
        // State 70
        0,
        // State 71
        0,
        // State 72
        0,
        // State 73
        0,
        // State 74
        0,
        // State 75
        0,
        // State 76
        0,
        // State 77
        0,
        // State 78
        0,
        // State 79
        0,
        // State 80
        -45,
        // State 81
        -43,
        // State 82
        0,
        // State 83
        0,
        // State 84
        0,
        // State 85
        0,
        // State 86
        0,
        // State 87
        0,
        // State 88
        0,
        // State 89
        0,
        // State 90
        0,
        // State 91
        0,
        // State 92
        0,
        // State 93
        0,
        // State 94
        0,
    ];
    fn __goto(state: i8, nt: usize) -> i8 {
        match nt {
            2 => 25,
            3 => match state {
                11 | 21 => 69,
                17 => 78,
                22 => 88,
                24 => 93,
                _ => 39,
            },
            4 => match state {
                16 => 77,
                _ => 52,
            },
            5 => match state {
                10 => 68,
                13 => 71,
                20 => 85,
                _ => 53,
            },
            6 => 54,
            7 => match state {
                23 => 91,
                _ => 73,
            },
            8 => 74,
            9 => 26,
            10 => match state {
                9 => 67,
                12 => 70,
                19 => 84,
                _ => 40,
            },
            11 => match state {
                7 => 61,
                _ => 41,
            },
            12 => match state {
                0 => 27,
                8 => 63,
                _ => 48,
            },
            13 => 8,
            14 => match state {
                15 => 76,
                _ => 49,
            },
            15 => 28,
            16 => 50,
            17 => 51,
            18 => match state {
                0 | 3 | 8 | 15 => 29,
                1 => 38,
                5 => 57,
                6 => 60,
                18 => 79,
                _ => 42,
            },
            19 => 58,
            20 => 30,
            _ => 0,
        }
    }
    #[allow(clippy::needless_raw_string_hashes)]
    const __TERMINAL: &[&str] = &[
        r###""ident""###,
        r###""string_lit""###,
        r###""float_lit""###,
        r###""bool_lit""###,
        r###""int_lit""###,
        r###""[""###,
        r###""]""###,
        r###""(""###,
        r###"")""###,
        r###""{""###,
        r###""}""###,
        r###"",""###,
        r###""<""###,
        r###"">""###,
        r###"":""###,
        r###""#""###,
        r###""@""###,
        r###""::""###,
        r###""map""###,
    ];
    fn __expected_tokens(__state: i8) -> alloc::vec::Vec<alloc::string::String> {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            let next_state = __action(__state, index);
            if next_state == 0 {
                None
            } else {
                Some(alloc::string::ToString::to_string(terminal))
            }
        }).collect()
    }
    fn __expected_tokens_from_states<
        '__0,
    >(
        __states: &[i8],
        _: core::marker::PhantomData<()>,
    ) -> alloc::vec::Vec<alloc::string::String>
    {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            if __accepts(None, __states, Some(index), core::marker::PhantomData::<()>) {
                Some(alloc::string::ToString::to_string(terminal))
            } else {
                None
            }
        }).collect()
    }
    struct __StateMachine<'__0>
    where 
    {
        ast: &'__0 RefCell<SceneBsnAst>,
        __phantom: core::marker::PhantomData<()>,
    }
    impl<'__0> __state_machine::ParserDefinition for __StateMachine<'__0>
    where 
    {
        type Location = usize;
        type Error = lexer::Error;
        type Token = lexer::Token;
        type TokenIndex = usize;
        type Symbol = __Symbol<>;
        type Success = Entity;
        type StateIndex = i8;
        type Action = i8;
        type ReduceIndex = i8;
        type NonterminalIndex = usize;

        #[inline]
        fn start_location(&self) -> Self::Location {
              Default::default()
        }

        #[inline]
        fn start_state(&self) -> Self::StateIndex {
              0
        }

        #[inline]
        fn token_to_index(&self, token: &Self::Token) -> Option<usize> {
            __token_to_integer(token, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn action(&self, state: i8, integer: usize) -> i8 {
            __action(state, integer)
        }

        #[inline]
        fn error_action(&self, state: i8) -> i8 {
            __action(state, 19 - 1)
        }

        #[inline]
        fn eof_action(&self, state: i8) -> i8 {
            __EOF_ACTION[state as usize]
        }

        #[inline]
        fn goto(&self, state: i8, nt: usize) -> i8 {
            __goto(state, nt)
        }

        fn token_to_symbol(&self, token_index: usize, token: Self::Token) -> Self::Symbol {
            __token_to_symbol(token_index, token, core::marker::PhantomData::<()>)
        }

        fn expected_tokens(&self, state: i8) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens(state)
        }

        fn expected_tokens_from_states(&self, states: &[i8]) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens_from_states(states, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn uses_error_recovery(&self) -> bool {
            false
        }

        #[inline]
        fn error_recovery_symbol(
            &self,
            recovery: __state_machine::ErrorRecovery<Self>,
        ) -> Self::Symbol {
            panic!("error recovery not enabled for this grammar")
        }

        fn reduce(
            &mut self,
            action: i8,
            start_location: Option<&Self::Location>,
            states: &mut alloc::vec::Vec<i8>,
            symbols: &mut alloc::vec::Vec<__state_machine::SymbolTriple<Self>>,
        ) -> Option<__state_machine::ParseResult<Self>> {
            __reduce(
                self.ast,
                action,
                start_location,
                states,
                symbols,
                core::marker::PhantomData::<()>,
            )
        }

        fn simulate_reduce(&self, action: i8) -> __state_machine::SimulatedReduce<Self> {
            __simulate_reduce(action, core::marker::PhantomData::<()>)
        }
    }
    fn __token_to_integer<
    >(
        __token: &lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> Option<usize>
    {
        #[warn(unused_variables)]
        match __token {
            lexer::Token::Ident(_) if true => Some(0),
            lexer::Token::StringLit(_) if true => Some(1),
            lexer::Token::FloatLit(_) if true => Some(2),
            lexer::Token::BoolLit(_) if true => Some(3),
            lexer::Token::IntLit(_) if true => Some(4),
            lexer::Token::LBracket if true => Some(5),
            lexer::Token::RBracket if true => Some(6),
            lexer::Token::LParen if true => Some(7),
            lexer::Token::RParen if true => Some(8),
            lexer::Token::LBrace if true => Some(9),
            lexer::Token::RBrace if true => Some(10),
            lexer::Token::Comma if true => Some(11),
            lexer::Token::Less if true => Some(12),
            lexer::Token::Greater if true => Some(13),
            lexer::Token::Colon if true => Some(14),
            lexer::Token::Hash if true => Some(15),
            lexer::Token::At if true => Some(16),
            lexer::Token::DoubleColon if true => Some(17),
            lexer::Token::Map if true => Some(18),
            _ => None,
        }
    }
    fn __token_to_symbol<
    >(
        __token_index: usize,
        __token: lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> __Symbol<>
    {
        #[allow(clippy::manual_range_patterns)]match __token_index {
            0 | 1 => match __token {
                lexer::Token::Ident(__tok0) | lexer::Token::StringLit(__tok0) if true => __Symbol::Variant0(__tok0),
                _ => unreachable!(),
            },
            2 => match __token {
                lexer::Token::FloatLit(__tok0) if true => __Symbol::Variant1(__tok0),
                _ => unreachable!(),
            },
            3 => match __token {
                lexer::Token::BoolLit(__tok0) if true => __Symbol::Variant2(__tok0),
                _ => unreachable!(),
            },
            4 => match __token {
                lexer::Token::IntLit(__tok0) if true => __Symbol::Variant3(__tok0),
                _ => unreachable!(),
            },
            5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 => __Symbol::Variant4(__token),
            _ => unreachable!(),
        }
    }
    fn __simulate_reduce<
        '__0,
    >(
        __reduce_index: i8,
        _: core::marker::PhantomData<()>,
    ) -> __state_machine::SimulatedReduce<__StateMachine<'__0>>
    {
        match __reduce_index {
            0 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 0,
                }
            }
            1 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 0,
                }
            }
            2 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 1,
                }
            }
            3 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 1,
                }
            }
            4 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 2,
                }
            }
            5 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            6 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            7 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            8 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            9 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            10 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            11 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            12 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            13 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            14 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            15 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            16 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            17 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            18 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            19 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            20 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            21 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 4,
                }
            }
            22 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 5,
                }
            }
            23 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 5,
                }
            }
            24 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 5,
                }
            }
            25 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 6,
                }
            }
            26 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 6,
                }
            }
            27 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 7,
                }
            }
            28 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 8,
                }
            }
            29 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 8,
                }
            }
            30 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            31 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            32 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 10,
                }
            }
            33 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 10,
                }
            }
            34 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 10,
                }
            }
            35 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 11,
                }
            }
            36 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 11,
                }
            }
            37 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            38 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            39 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            40 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 12,
                }
            }
            41 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            42 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            43 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            44 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            45 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            46 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 13,
                }
            }
            47 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 13,
                }
            }
            48 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 14,
                }
            }
            49 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 15,
                }
            }
            50 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 16,
                }
            }
            51 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 16,
                }
            }
            52 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 16,
                }
            }
            53 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 17,
                }
            }
            54 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 17,
                }
            }
            55 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 18,
                }
            }
            56 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 18,
                }
            }
            57 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 19,
                }
            }
            58 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 19,
                }
            }
            59 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 20,
                }
            }
            60 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 20,
                }
            }
            61 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 21,
                }
            }
            62 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 21,
                }
            }
            63 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 22,
                }
            }
            64 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 22,
                }
            }
            65 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 22,
                }
            }
            66 => __state_machine::SimulatedReduce::Accept,
            67 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 24,
                }
            }
            68 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 25,
                }
            }
            _ => panic!("invalid reduction index {__reduce_index}")
        }
    }
    pub struct PatchParser {
        _priv: (),
    }

    impl Default for PatchParser { fn default() -> Self { Self::new() } }
    impl PatchParser {
        pub fn new() -> PatchParser {
            PatchParser {
                _priv: (),
            }
        }

        #[allow(dead_code)]
        pub fn parse<
            __TOKEN: __ToTriple<>,
            __TOKENS: IntoIterator<Item=__TOKEN>,
        >(
            &self,
            ast: &RefCell<SceneBsnAst>,
            __tokens0: __TOKENS,
        ) -> Result<Entity, __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>
        {
            let __tokens = __tokens0.into_iter();
            let mut __tokens = __tokens.map(|t| __ToTriple::to_triple(t));
            __state_machine::Parser::drive(
                __StateMachine {
                    ast,
                    __phantom: core::marker::PhantomData::<()>,
                },
                __tokens,
            )
        }
    }
    fn __accepts<
        '__0,
    >(
        __error_state: Option<i8>,
        __states: &[i8],
        __opt_integer: Option<usize>,
        _: core::marker::PhantomData<()>,
    ) -> bool
    {
        let mut __states = __states.to_vec();
        __states.extend(__error_state);
        loop {
            let mut __states_len = __states.len();
            let __top = __states[__states_len - 1];
            let __action = match __opt_integer {
                None => __EOF_ACTION[__top as usize],
                Some(__integer) => __action(__top, __integer),
            };
            if __action == 0 { return false; }
            if __action > 0 { return true; }
            let (__to_pop, __nt) = match __simulate_reduce(-(__action + 1), core::marker::PhantomData::<()>) {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop, nonterminal_produced
                } => (states_to_pop, nonterminal_produced),
                __state_machine::SimulatedReduce::Accept => return true,
            };
            __states_len -= __to_pop;
            __states.truncate(__states_len);
            let __top = __states[__states_len - 1];
            let __next_state = __goto(__top, __nt);
            __states.push(__next_state);
        }
    }
    fn __reduce<
    >(
        ast: &RefCell<SceneBsnAst>,
        __action: i8,
        __lookahead_start: Option<&usize>,
        __states: &mut alloc::vec::Vec<i8>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> Option<Result<Entity,__lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>>
    {
        let (__pop_states, __nonterminal) = match __action {
            0 => {
                __reduce0(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            1 => {
                __reduce1(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            2 => {
                __reduce2(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            3 => {
                __reduce3(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            4 => {
                __reduce4(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            5 => {
                __reduce5(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            6 => {
                __reduce6(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            7 => {
                __reduce7(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            8 => {
                __reduce8(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            9 => {
                __reduce9(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            10 => {
                __reduce10(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            11 => {
                __reduce11(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            12 => {
                __reduce12(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            13 => {
                __reduce13(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            14 => {
                __reduce14(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            15 => {
                __reduce15(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            16 => {
                __reduce16(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            17 => {
                __reduce17(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            18 => {
                __reduce18(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            19 => {
                __reduce19(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            20 => {
                __reduce20(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            21 => {
                __reduce21(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            22 => {
                __reduce22(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            23 => {
                __reduce23(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            24 => {
                __reduce24(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            25 => {
                __reduce25(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            26 => {
                __reduce26(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            27 => {
                __reduce27(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            28 => {
                __reduce28(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            29 => {
                __reduce29(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            30 => {
                __reduce30(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            31 => {
                __reduce31(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            32 => {
                __reduce32(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            33 => {
                __reduce33(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            34 => {
                __reduce34(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            35 => {
                __reduce35(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            36 => {
                __reduce36(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            37 => {
                __reduce37(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            38 => {
                __reduce38(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            39 => {
                __reduce39(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            40 => {
                __reduce40(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            41 => {
                __reduce41(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            42 => {
                __reduce42(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            43 => {
                __reduce43(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            44 => {
                __reduce44(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            45 => {
                __reduce45(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            46 => {
                __reduce46(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            47 => {
                __reduce47(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            48 => {
                __reduce48(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            49 => {
                __reduce49(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            50 => {
                __reduce50(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            51 => {
                __reduce51(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            52 => {
                __reduce52(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            53 => {
                __reduce53(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            54 => {
                __reduce54(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            55 => {
                __reduce55(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            56 => {
                __reduce56(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            57 => {
                __reduce57(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            58 => {
                __reduce58(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            59 => {
                __reduce59(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            60 => {
                __reduce60(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            61 => {
                __reduce61(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            62 => {
                __reduce62(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            63 => {
                __reduce63(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            64 => {
                __reduce64(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            65 => {
                __reduce65(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            66 => {
                // __Patch = Patch => ActionFn(2);
                let __sym0 = __pop_Variant12(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action2::<>(ast, __sym0);
                return Some(Ok(__nt));
            }
            67 => {
                __reduce67(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            68 => {
                __reduce68(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            _ => panic!("invalid action code {__action}")
        };
        let __states_len = __states.len();
        __states.truncate(__states_len - __pop_states);
        let __state = *__states.last().unwrap();
        let __next_state = __goto(__state, __nonterminal);
        __states.push(__next_state);
        None
    }
    #[inline(never)]
    fn __symbol_type_mismatch() -> ! {
        panic!("symbol type mismatch")
    }
    fn __pop_Variant9<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, (BsnValue, BsnValue), usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant9(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant7<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnField, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant7(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant6<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnValue, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant6(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant12<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Entity, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant12(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant5<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Option<lexer::Token>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant5(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant0<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, String, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant0(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant10<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<(BsnValue, BsnValue)>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant10(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant8<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnField>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant8(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant11<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnValue>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant11(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant14<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant14(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant13<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, alloc::vec::Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant13(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant2<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, bool, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant2(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant1<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, f64, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant1(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant3<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, i128, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant3(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant4<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, lexer::Token, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant4(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __reduce0<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? = "," => ActionFn(55);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action55::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 0)
    }
    fn __reduce1<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? =  => ActionFn(56);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action56::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 0)
    }
    fn __reduce2<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? = "@" => ActionFn(51);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action51::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 1)
    }
    fn __reduce3<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? =  => ActionFn(52);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action52::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 1)
    }
    fn __reduce4<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Base = ":", "string_lit" => ActionFn(16);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action16::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 2)
    }
    fn __reduce5<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol => ActionFn(69);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action69::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce6<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol => ActionFn(70);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action70::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce7<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "{", Fields, "}" => ActionFn(71);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action71::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce8<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "{", Fields, "}" => ActionFn(72);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action72::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce9<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(73);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action73::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce10<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "(", NamedTupleArgs, ")" => ActionFn(74);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action74::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce11<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "string_lit" => ActionFn(35);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action35::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce12<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "float_lit" => ActionFn(36);
        let __sym0 = __pop_Variant1(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action36::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce13<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "bool_lit" => ActionFn(37);
        let __sym0 = __pop_Variant2(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action37::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce14<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "int_lit" => ActionFn(38);
        let __sym0 = __pop_Variant3(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action38::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce15<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, ",", "]" => ActionFn(57);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action57::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce16<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, "]" => ActionFn(58);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action58::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce17<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", "]" => ActionFn(40);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action40::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce18<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, ",", "]" => ActionFn(59);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action59::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce19<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, "]" => ActionFn(60);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action60::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce20<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", "]" => ActionFn(42);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action42::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce21<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Field = "ident", ":", Expr => ActionFn(27);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action27::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant7(__nt), __end));
        (3, 4)
    }
    fn __reduce22<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields =  => ActionFn(23);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action23::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (0, 5)
    }
    fn __reduce23<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner, "," => ActionFn(61);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action61::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (2, 5)
    }
    fn __reduce24<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner => ActionFn(62);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action62::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 5)
    }
    fn __reduce25<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = Field => ActionFn(25);
        let __sym0 = __pop_Variant7(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action25::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 6)
    }
    fn __reduce26<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = FieldsInner, ",", Field => ActionFn(26);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant7(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action26::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (3, 6)
    }
    fn __reduce27<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPair = "(", Expr, ",", Expr, ")" => ActionFn(45);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant6(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant6(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action45::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant9(__nt), __end));
        (5, 7)
    }
    fn __reduce28<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPair => ActionFn(43);
        let __sym0 = __pop_Variant9(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action43::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (1, 8)
    }
    fn __reduce29<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPairsInner, ",", MapPair => ActionFn(44);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant9(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant10(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action44::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (3, 8)
    }
    fn __reduce30<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "ident" => ActionFn(14);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action14::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce31<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "string_lit" => ActionFn(15);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action15::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce32<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs =  => ActionFn(28);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action28::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (0, 10)
    }
    fn __reduce33<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner, "," => ActionFn(63);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action63::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (2, 10)
    }
    fn __reduce34<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner => ActionFn(64);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action64::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 10)
    }
    fn __reduce35<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = Expr => ActionFn(30);
        let __sym0 = __pop_Variant6(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action30::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 11)
    }
    fn __reduce36<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = NamedTupleArgsInner, ",", Expr => ActionFn(31);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action31::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (3, 11)
    }
    fn __reduce37<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Name => ActionFn(8);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action8::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce38<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Base => ActionFn(9);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action9::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce39<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Relation => ActionFn(10);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action10::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce40<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol => ActionFn(75);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action75::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 12)
    }
    fn __reduce41<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol => ActionFn(76);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action76::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce42<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "{", Fields, "}" => ActionFn(77);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action77::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce43<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "{", Fields, "}" => ActionFn(78);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action78::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce44<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(79);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action79::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce45<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "(", NamedTupleArgs, ")" => ActionFn(80);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action80::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce46<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch => ActionFn(53);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action53::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (1, 13)
    }
    fn __reduce47<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch+, Patch => ActionFn(54);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant12(__symbols);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action54::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (2, 13)
    }
    fn __reduce48<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patches = Patch+ => ActionFn(7);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action7::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 14)
    }
    fn __reduce49<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relation = Symbol, "[", Relations, "]" => ActionFn(46);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant14(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action46::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (4, 15)
    }
    fn __reduce50<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations =  => ActionFn(47);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action47::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (0, 16)
    }
    fn __reduce51<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner, "," => ActionFn(65);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action65::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (2, 16)
    }
    fn __reduce52<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner => ActionFn(66);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action66::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 16)
    }
    fn __reduce53<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = RelationsInner, ",", Patches => ActionFn(49);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action49::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 17)
    }
    fn __reduce54<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = Patches => ActionFn(50);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action50::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 17)
    }
    fn __reduce55<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath => ActionFn(17);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action17::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 18)
    }
    fn __reduce56<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath, "<", SymbolArgs, ">" => ActionFn(18);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action18::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (4, 18)
    }
    fn __reduce57<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = Symbol => ActionFn(21);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action21::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 19)
    }
    fn __reduce58<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = SymbolArgs, ",", Symbol => ActionFn(22);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action22::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 19)
    }
    fn __reduce59<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = SymbolPath, "::", "ident" => ActionFn(19);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action19::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 20)
    }
    fn __reduce60<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = "ident" => ActionFn(20);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action20::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 20)
    }
    fn __reduce61<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = Patches => ActionFn(5);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action5::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 21)
    }
    fn __reduce62<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = TopLevelGroups, ",", Patches => ActionFn(6);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action6::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 21)
    }
    fn __reduce63<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches =  => ActionFn(3);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action3::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (0, 22)
    }
    fn __reduce64<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups, "," => ActionFn(67);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action67::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 22)
    }
    fn __reduce65<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups => ActionFn(68);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action68::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 22)
    }
    fn __reduce67<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __Patches = Patches => ActionFn(1);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action1::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 24)
    }
    fn __reduce68<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __TopLevelPatches = TopLevelPatches => ActionFn(0);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action0::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 25)
    }
}
#[allow(unused_imports)]
pub use self::__parse__Patch::PatchParser;

#[rustfmt::skip]
#[allow(explicit_outlives_requirements, non_snake_case, non_camel_case_types, unused_mut, unused_variables, unused_imports, unused_parens, clippy::needless_lifetimes, clippy::type_complexity, clippy::needless_return, clippy::too_many_arguments, clippy::match_single_binding, clippy::clone_on_copy, clippy::unit_arg)]
mod __parse__Patches {

    use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
    use crate::parse::lexer;
    use bevy::ecs::entity::Entity;
    use std::cell::RefCell;
    #[allow(unused_extern_crates)]
    extern crate lalrpop_util as __lalrpop_util;
    #[allow(unused_imports)]
    use self::__lalrpop_util::state_machine as __state_machine;
    #[allow(unused_extern_crates)]
    extern crate alloc;
    use super::__ToTriple;
    #[allow(dead_code)]
    pub(crate) enum __Symbol<>
     {
        Variant0(String),
        Variant1(f64),
        Variant2(bool),
        Variant3(i128),
        Variant4(lexer::Token),
        Variant5(Option<lexer::Token>),
        Variant6(BsnValue),
        Variant7(BsnField),
        Variant8(Vec<BsnField>),
        Variant9((BsnValue, BsnValue)),
        Variant10(Vec<(BsnValue, BsnValue)>),
        Variant11(Vec<BsnValue>),
        Variant12(Entity),
        Variant13(alloc::vec::Vec<Entity>),
        Variant14(Vec<Entity>),
    }
    const __ACTION: &[i8] = &[
        // State 0
        35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 33, 3, 0, 0,
        // State 1
        35, 0, 0, 0, 0, 0, -49, 0, 0, 0, 0, -49, 0, 0, 34, 33, 3, 0, 0,
        // State 2
        35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 3
        35, 50, 47, 46, 48, 9, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 4
        35, 0, 0, 0, 0, 0, -51, 0, 0, 0, 0, 0, 0, 0, 34, 33, 3, 0, 0,
        // State 5
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 6
        35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 7
        35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 8
        35, 50, 47, 46, 48, 9, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 9
        35, 50, 47, 46, 48, 9, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 10
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 11
        35, 50, 47, 46, 48, 9, 0, 0, -34, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 12
        35, 50, 47, 46, 48, 9, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 13
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 14
        0, 0, 0, 0, 0, 0, 76, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 15
        35, 0, 0, 0, 0, 0, -52, 0, 0, 0, 0, 0, 0, 0, 34, 33, 3, 0, 0,
        // State 16
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, -24, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 17
        35, 50, 47, 46, 48, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 18
        35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 19
        35, 50, 47, 46, 48, 9, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 20
        57, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 21
        35, 50, 47, 46, 48, 9, 87, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 22
        35, 50, 47, 46, 48, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 23
        0, 0, 0, 0, 0, 0, 93, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 24
        35, 50, 47, 46, 48, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 49,
        // State 25
        -39, 0, 0, 0, 0, 0, -39, 0, 0, 0, 0, -39, 0, 0, -39, -39, -39, 0, 0,
        // State 26
        -38, 0, 0, 0, 0, 0, -38, 0, 0, 0, 0, -38, 0, 0, -38, -38, -38, 0, 0,
        // State 27
        -47, 0, 0, 0, 0, 0, -47, 0, 0, 0, 0, -47, 0, 0, -47, -47, -47, 0, 0,
        // State 28
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 29
        -40, 0, 0, 0, 0, 0, -40, 0, 0, 0, 0, -40, 0, 0, -40, -40, -40, 0, 0,
        // State 30
        -42, 0, 0, 0, 0, 5, -42, 4, 0, 6, 0, -42, 0, 0, -42, -42, -42, 0, 0,
        // State 31
        -56, 0, 0, 0, 0, -56, -56, -56, -56, -56, -56, -56, 7, -56, -56, -56, -56, 37, 0,
        // State 32
        38, 39, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 33
        0, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 34
        -61, 0, 0, 0, 0, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, 0,
        // State 35
        -48, 0, 0, 0, 0, 0, -48, 0, 0, 0, 0, -48, 0, 0, -48, -48, -48, 0, 0,
        // State 36
        58, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 37
        -31, 0, 0, 0, 0, 0, -31, 0, 0, 0, 0, -31, 0, 0, -31, -31, -31, 0, 0,
        // State 38
        -32, 0, 0, 0, 0, 0, -32, 0, 0, 0, 0, -32, 0, 0, -32, -32, -32, 0, 0,
        // State 39
        -5, 0, 0, 0, 0, 0, -5, 0, 0, 0, 0, -5, 0, 0, -5, -5, -5, 0, 0,
        // State 40
        -41, 0, 0, 0, 0, 0, -41, 10, 0, 11, 0, -41, 0, 0, -41, -41, -41, 0, 0,
        // State 41
        0, 0, 0, 0, 0, 0, -36, 0, -36, 0, 0, -36, 0, 0, 0, 0, 0, 0, 0,
        // State 42
        0, 0, 0, 0, 0, 0, 0, 0, 61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 43
        0, 0, 0, 0, 0, 0, 0, 0, -35, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0,
        // State 44
        0, 0, 0, 0, 0, 0, -7, 13, -7, 14, -7, -7, 0, 0, 0, 0, 0, 0, 0,
        // State 45
        0, 0, 0, 0, 0, 0, -14, 0, -14, 0, -14, -14, 0, 0, 0, 0, 0, 0, 0,
        // State 46
        0, 0, 0, 0, 0, 0, -13, 0, -13, 0, -13, -13, 0, 0, 0, 0, 0, 0, 0,
        // State 47
        0, 0, 0, 0, 0, 0, -15, 0, -15, 0, -15, -15, 0, 0, 0, 0, 0, 0, 0,
        // State 48
        0, 0, 0, 0, 0, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 49
        0, 0, 0, 0, 0, 0, -12, 0, -12, 0, -12, -12, 0, 0, 0, 0, 0, 0, 0,
        // State 50
        0, 0, 0, 0, 0, 0, -55, 0, 0, 0, 0, -55, 0, 0, 0, 0, 0, 0, 0,
        // State 51
        0, 0, 0, 0, 0, 0, 65, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 52
        0, 0, 0, 0, 0, 0, -53, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0,
        // State 53
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -26, -26, 0, 0, 0, 0, 0, 0, 0,
        // State 54
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 66, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 55
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -25, 17, 0, 0, 0, 0, 0, 0, 0,
        // State 56
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 18, 0, 0, 0, 0,
        // State 57
        -60, 0, 0, 0, 0, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, 0,
        // State 58
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -58, 0, -58, 0, 0, 0, 0, 0,
        // State 59
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 19, 0, 67, 0, 0, 0, 0, 0,
        // State 60
        -46, 0, 0, 0, 0, 0, -46, 0, 0, 0, 0, -46, 0, 0, -46, -46, -46, 0, 0,
        // State 61
        0, 0, 0, 0, 0, 0, -6, 20, -6, 21, -6, -6, 0, 0, 0, 0, 0, 0, 0,
        // State 62
        0, 0, 0, 0, 0, 0, 73, 0, 0, 0, 0, 22, 0, 0, 0, 0, 0, 0, 0,
        // State 63
        0, 0, 0, 0, 0, 0, -18, 0, -18, 0, -18, -18, 0, 0, 0, 0, 0, 0, 0,
        // State 64
        -50, 0, 0, 0, 0, 0, -50, 0, 0, 0, 0, -50, 0, 0, -50, -50, -50, 0, 0,
        // State 65
        -44, 0, 0, 0, 0, 0, -44, 0, 0, 0, 0, -44, 0, 0, -44, -44, -44, 0, 0,
        // State 66
        -57, 0, 0, 0, 0, -57, -57, -57, -57, -57, -57, -57, 0, -57, -57, -57, -57, 0, 0,
        // State 67
        0, 0, 0, 0, 0, 0, 0, 0, 81, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 68
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 82, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 69
        0, 0, 0, 0, 0, 0, -37, 0, -37, 0, 0, -37, 0, 0, 0, 0, 0, 0, 0,
        // State 70
        0, 0, 0, 0, 0, 0, 0, 0, 83, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 71
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 84, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 72
        0, 0, 0, 0, 0, 0, -17, 0, -17, 0, -17, -17, 0, 0, 0, 0, 0, 0, 0,
        // State 73
        0, 0, 0, 0, 0, 0, -29, 0, 0, 0, 0, -29, 0, 0, 0, 0, 0, 0, 0,
        // State 74
        0, 0, 0, 0, 0, 0, 88, 0, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0,
        // State 75
        0, 0, 0, 0, 0, 0, -21, 0, -21, 0, -21, -21, 0, 0, 0, 0, 0, 0, 0,
        // State 76
        0, 0, 0, 0, 0, 0, -54, 0, 0, 0, 0, -54, 0, 0, 0, 0, 0, 0, 0,
        // State 77
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -27, -27, 0, 0, 0, 0, 0, 0, 0,
        // State 78
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -22, -22, 0, 0, 0, 0, 0, 0, 0,
        // State 79
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -59, 0, -59, 0, 0, 0, 0, 0,
        // State 80
        -45, 0, 0, 0, 0, 0, -45, 0, 0, 0, 0, -45, 0, 0, -45, -45, -45, 0, 0,
        // State 81
        -43, 0, 0, 0, 0, 0, -43, 0, 0, 0, 0, -43, 0, 0, -43, -43, -43, 0, 0,
        // State 82
        0, 0, 0, 0, 0, 0, -11, 0, -11, 0, -11, -11, 0, 0, 0, 0, 0, 0, 0,
        // State 83
        0, 0, 0, 0, 0, 0, -9, 0, -9, 0, -9, -9, 0, 0, 0, 0, 0, 0, 0,
        // State 84
        0, 0, 0, 0, 0, 0, 0, 0, 90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 85
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 91, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 86
        0, 0, 0, 0, 0, 0, -16, 0, -16, 0, -16, -16, 0, 0, 0, 0, 0, 0, 0,
        // State 87
        0, 0, 0, 0, 0, 0, -20, 0, -20, 0, -20, -20, 0, 0, 0, 0, 0, 0, 0,
        // State 88
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 25, 0, 0, 0, 0, 0, 0, 0,
        // State 89
        0, 0, 0, 0, 0, 0, -10, 0, -10, 0, -10, -10, 0, 0, 0, 0, 0, 0, 0,
        // State 90
        0, 0, 0, 0, 0, 0, -8, 0, -8, 0, -8, -8, 0, 0, 0, 0, 0, 0, 0,
        // State 91
        0, 0, 0, 0, 0, 0, -30, 0, 0, 0, 0, -30, 0, 0, 0, 0, 0, 0, 0,
        // State 92
        0, 0, 0, 0, 0, 0, -19, 0, -19, 0, -19, -19, 0, 0, 0, 0, 0, 0, 0,
        // State 93
        0, 0, 0, 0, 0, 0, 0, 0, 95, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 94
        0, 0, 0, 0, 0, 0, -28, 0, 0, 0, 0, -28, 0, 0, 0, 0, 0, 0, 0,
    ];
    fn __action(state: i8, integer: usize) -> i8 {
        __ACTION[(state as usize) * 19 + integer]
    }
    const __EOF_ACTION: &[i8] = &[
        // State 0
        0,
        // State 1
        -49,
        // State 2
        0,
        // State 3
        0,
        // State 4
        0,
        // State 5
        0,
        // State 6
        0,
        // State 7
        0,
        // State 8
        0,
        // State 9
        0,
        // State 10
        0,
        // State 11
        0,
        // State 12
        0,
        // State 13
        0,
        // State 14
        0,
        // State 15
        0,
        // State 16
        0,
        // State 17
        0,
        // State 18
        0,
        // State 19
        0,
        // State 20
        0,
        // State 21
        0,
        // State 22
        0,
        // State 23
        0,
        // State 24
        0,
        // State 25
        -39,
        // State 26
        -38,
        // State 27
        -47,
        // State 28
        -68,
        // State 29
        -40,
        // State 30
        -42,
        // State 31
        -56,
        // State 32
        0,
        // State 33
        0,
        // State 34
        -61,
        // State 35
        -48,
        // State 36
        0,
        // State 37
        -31,
        // State 38
        -32,
        // State 39
        -5,
        // State 40
        -41,
        // State 41
        0,
        // State 42
        0,
        // State 43
        0,
        // State 44
        0,
        // State 45
        0,
        // State 46
        0,
        // State 47
        0,
        // State 48
        0,
        // State 49
        0,
        // State 50
        0,
        // State 51
        0,
        // State 52
        0,
        // State 53
        0,
        // State 54
        0,
        // State 55
        0,
        // State 56
        0,
        // State 57
        -60,
        // State 58
        0,
        // State 59
        0,
        // State 60
        -46,
        // State 61
        0,
        // State 62
        0,
        // State 63
        0,
        // State 64
        -50,
        // State 65
        -44,
        // State 66
        -57,
        // State 67
        0,
        // State 68
        0,
        // State 69
        0,
        // State 70
        0,
        // State 71
        0,
        // State 72
        0,
        // State 73
        0,
        // State 74
        0,
        // State 75
        0,
        // State 76
        0,
        // State 77
        0,
        // State 78
        0,
        // State 79
        0,
        // State 80
        -45,
        // State 81
        -43,
        // State 82
        0,
        // State 83
        0,
        // State 84
        0,
        // State 85
        0,
        // State 86
        0,
        // State 87
        0,
        // State 88
        0,
        // State 89
        0,
        // State 90
        0,
        // State 91
        0,
        // State 92
        0,
        // State 93
        0,
        // State 94
        0,
    ];
    fn __goto(state: i8, nt: usize) -> i8 {
        match nt {
            2 => 25,
            3 => match state {
                11 | 21 => 69,
                17 => 78,
                22 => 88,
                24 => 93,
                _ => 41,
            },
            4 => match state {
                16 => 77,
                _ => 53,
            },
            5 => match state {
                10 => 68,
                13 => 71,
                20 => 85,
                _ => 54,
            },
            6 => 55,
            7 => match state {
                23 => 91,
                _ => 73,
            },
            8 => 74,
            9 => 26,
            10 => match state {
                9 => 67,
                12 => 70,
                19 => 84,
                _ => 42,
            },
            11 => match state {
                8 => 62,
                _ => 43,
            },
            12 => match state {
                1 => 35,
                _ => 27,
            },
            13 => 1,
            14 => match state {
                4 => 50,
                15 => 76,
                _ => 28,
            },
            15 => 29,
            16 => 51,
            17 => 52,
            18 => match state {
                0..=1 | 4 | 15 => 30,
                2 => 40,
                6 => 58,
                7 => 61,
                18 => 79,
                _ => 44,
            },
            19 => 59,
            20 => 31,
            _ => 0,
        }
    }
    #[allow(clippy::needless_raw_string_hashes)]
    const __TERMINAL: &[&str] = &[
        r###""ident""###,
        r###""string_lit""###,
        r###""float_lit""###,
        r###""bool_lit""###,
        r###""int_lit""###,
        r###""[""###,
        r###""]""###,
        r###""(""###,
        r###"")""###,
        r###""{""###,
        r###""}""###,
        r###"",""###,
        r###""<""###,
        r###"">""###,
        r###"":""###,
        r###""#""###,
        r###""@""###,
        r###""::""###,
        r###""map""###,
    ];
    fn __expected_tokens(__state: i8) -> alloc::vec::Vec<alloc::string::String> {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            let next_state = __action(__state, index);
            if next_state == 0 {
                None
            } else {
                Some(alloc::string::ToString::to_string(terminal))
            }
        }).collect()
    }
    fn __expected_tokens_from_states<
        '__0,
    >(
        __states: &[i8],
        _: core::marker::PhantomData<()>,
    ) -> alloc::vec::Vec<alloc::string::String>
    {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            if __accepts(None, __states, Some(index), core::marker::PhantomData::<()>) {
                Some(alloc::string::ToString::to_string(terminal))
            } else {
                None
            }
        }).collect()
    }
    struct __StateMachine<'__0>
    where 
    {
        ast: &'__0 RefCell<SceneBsnAst>,
        __phantom: core::marker::PhantomData<()>,
    }
    impl<'__0> __state_machine::ParserDefinition for __StateMachine<'__0>
    where 
    {
        type Location = usize;
        type Error = lexer::Error;
        type Token = lexer::Token;
        type TokenIndex = usize;
        type Symbol = __Symbol<>;
        type Success = Entity;
        type StateIndex = i8;
        type Action = i8;
        type ReduceIndex = i8;
        type NonterminalIndex = usize;

        #[inline]
        fn start_location(&self) -> Self::Location {
              Default::default()
        }

        #[inline]
        fn start_state(&self) -> Self::StateIndex {
              0
        }

        #[inline]
        fn token_to_index(&self, token: &Self::Token) -> Option<usize> {
            __token_to_integer(token, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn action(&self, state: i8, integer: usize) -> i8 {
            __action(state, integer)
        }

        #[inline]
        fn error_action(&self, state: i8) -> i8 {
            __action(state, 19 - 1)
        }

        #[inline]
        fn eof_action(&self, state: i8) -> i8 {
            __EOF_ACTION[state as usize]
        }

        #[inline]
        fn goto(&self, state: i8, nt: usize) -> i8 {
            __goto(state, nt)
        }

        fn token_to_symbol(&self, token_index: usize, token: Self::Token) -> Self::Symbol {
            __token_to_symbol(token_index, token, core::marker::PhantomData::<()>)
        }

        fn expected_tokens(&self, state: i8) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens(state)
        }

        fn expected_tokens_from_states(&self, states: &[i8]) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens_from_states(states, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn uses_error_recovery(&self) -> bool {
            false
        }

        #[inline]
        fn error_recovery_symbol(
            &self,
            recovery: __state_machine::ErrorRecovery<Self>,
        ) -> Self::Symbol {
            panic!("error recovery not enabled for this grammar")
        }

        fn reduce(
            &mut self,
            action: i8,
            start_location: Option<&Self::Location>,
            states: &mut alloc::vec::Vec<i8>,
            symbols: &mut alloc::vec::Vec<__state_machine::SymbolTriple<Self>>,
        ) -> Option<__state_machine::ParseResult<Self>> {
            __reduce(
                self.ast,
                action,
                start_location,
                states,
                symbols,
                core::marker::PhantomData::<()>,
            )
        }

        fn simulate_reduce(&self, action: i8) -> __state_machine::SimulatedReduce<Self> {
            __simulate_reduce(action, core::marker::PhantomData::<()>)
        }
    }
    fn __token_to_integer<
    >(
        __token: &lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> Option<usize>
    {
        #[warn(unused_variables)]
        match __token {
            lexer::Token::Ident(_) if true => Some(0),
            lexer::Token::StringLit(_) if true => Some(1),
            lexer::Token::FloatLit(_) if true => Some(2),
            lexer::Token::BoolLit(_) if true => Some(3),
            lexer::Token::IntLit(_) if true => Some(4),
            lexer::Token::LBracket if true => Some(5),
            lexer::Token::RBracket if true => Some(6),
            lexer::Token::LParen if true => Some(7),
            lexer::Token::RParen if true => Some(8),
            lexer::Token::LBrace if true => Some(9),
            lexer::Token::RBrace if true => Some(10),
            lexer::Token::Comma if true => Some(11),
            lexer::Token::Less if true => Some(12),
            lexer::Token::Greater if true => Some(13),
            lexer::Token::Colon if true => Some(14),
            lexer::Token::Hash if true => Some(15),
            lexer::Token::At if true => Some(16),
            lexer::Token::DoubleColon if true => Some(17),
            lexer::Token::Map if true => Some(18),
            _ => None,
        }
    }
    fn __token_to_symbol<
    >(
        __token_index: usize,
        __token: lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> __Symbol<>
    {
        #[allow(clippy::manual_range_patterns)]match __token_index {
            0 | 1 => match __token {
                lexer::Token::Ident(__tok0) | lexer::Token::StringLit(__tok0) if true => __Symbol::Variant0(__tok0),
                _ => unreachable!(),
            },
            2 => match __token {
                lexer::Token::FloatLit(__tok0) if true => __Symbol::Variant1(__tok0),
                _ => unreachable!(),
            },
            3 => match __token {
                lexer::Token::BoolLit(__tok0) if true => __Symbol::Variant2(__tok0),
                _ => unreachable!(),
            },
            4 => match __token {
                lexer::Token::IntLit(__tok0) if true => __Symbol::Variant3(__tok0),
                _ => unreachable!(),
            },
            5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 => __Symbol::Variant4(__token),
            _ => unreachable!(),
        }
    }
    fn __simulate_reduce<
        '__0,
    >(
        __reduce_index: i8,
        _: core::marker::PhantomData<()>,
    ) -> __state_machine::SimulatedReduce<__StateMachine<'__0>>
    {
        match __reduce_index {
            0 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 0,
                }
            }
            1 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 0,
                }
            }
            2 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 1,
                }
            }
            3 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 1,
                }
            }
            4 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 2,
                }
            }
            5 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            6 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            7 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            8 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            9 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            10 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            11 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            12 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            13 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            14 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            15 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            16 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            17 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            18 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            19 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            20 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            21 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 4,
                }
            }
            22 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 5,
                }
            }
            23 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 5,
                }
            }
            24 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 5,
                }
            }
            25 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 6,
                }
            }
            26 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 6,
                }
            }
            27 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 7,
                }
            }
            28 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 8,
                }
            }
            29 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 8,
                }
            }
            30 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            31 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            32 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 10,
                }
            }
            33 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 10,
                }
            }
            34 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 10,
                }
            }
            35 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 11,
                }
            }
            36 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 11,
                }
            }
            37 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            38 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            39 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            40 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 12,
                }
            }
            41 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            42 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            43 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            44 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            45 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            46 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 13,
                }
            }
            47 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 13,
                }
            }
            48 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 14,
                }
            }
            49 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 15,
                }
            }
            50 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 16,
                }
            }
            51 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 16,
                }
            }
            52 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 16,
                }
            }
            53 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 17,
                }
            }
            54 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 17,
                }
            }
            55 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 18,
                }
            }
            56 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 18,
                }
            }
            57 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 19,
                }
            }
            58 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 19,
                }
            }
            59 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 20,
                }
            }
            60 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 20,
                }
            }
            61 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 21,
                }
            }
            62 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 21,
                }
            }
            63 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 22,
                }
            }
            64 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 22,
                }
            }
            65 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 22,
                }
            }
            66 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 23,
                }
            }
            67 => __state_machine::SimulatedReduce::Accept,
            68 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 25,
                }
            }
            _ => panic!("invalid reduction index {__reduce_index}")
        }
    }
    pub struct PatchesParser {
        _priv: (),
    }

    impl Default for PatchesParser { fn default() -> Self { Self::new() } }
    impl PatchesParser {
        pub fn new() -> PatchesParser {
            PatchesParser {
                _priv: (),
            }
        }

        #[allow(dead_code)]
        pub fn parse<
            __TOKEN: __ToTriple<>,
            __TOKENS: IntoIterator<Item=__TOKEN>,
        >(
            &self,
            ast: &RefCell<SceneBsnAst>,
            __tokens0: __TOKENS,
        ) -> Result<Entity, __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>
        {
            let __tokens = __tokens0.into_iter();
            let mut __tokens = __tokens.map(|t| __ToTriple::to_triple(t));
            __state_machine::Parser::drive(
                __StateMachine {
                    ast,
                    __phantom: core::marker::PhantomData::<()>,
                },
                __tokens,
            )
        }
    }
    fn __accepts<
        '__0,
    >(
        __error_state: Option<i8>,
        __states: &[i8],
        __opt_integer: Option<usize>,
        _: core::marker::PhantomData<()>,
    ) -> bool
    {
        let mut __states = __states.to_vec();
        __states.extend(__error_state);
        loop {
            let mut __states_len = __states.len();
            let __top = __states[__states_len - 1];
            let __action = match __opt_integer {
                None => __EOF_ACTION[__top as usize],
                Some(__integer) => __action(__top, __integer),
            };
            if __action == 0 { return false; }
            if __action > 0 { return true; }
            let (__to_pop, __nt) = match __simulate_reduce(-(__action + 1), core::marker::PhantomData::<()>) {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop, nonterminal_produced
                } => (states_to_pop, nonterminal_produced),
                __state_machine::SimulatedReduce::Accept => return true,
            };
            __states_len -= __to_pop;
            __states.truncate(__states_len);
            let __top = __states[__states_len - 1];
            let __next_state = __goto(__top, __nt);
            __states.push(__next_state);
        }
    }
    fn __reduce<
    >(
        ast: &RefCell<SceneBsnAst>,
        __action: i8,
        __lookahead_start: Option<&usize>,
        __states: &mut alloc::vec::Vec<i8>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> Option<Result<Entity,__lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>>
    {
        let (__pop_states, __nonterminal) = match __action {
            0 => {
                __reduce0(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            1 => {
                __reduce1(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            2 => {
                __reduce2(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            3 => {
                __reduce3(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            4 => {
                __reduce4(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            5 => {
                __reduce5(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            6 => {
                __reduce6(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            7 => {
                __reduce7(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            8 => {
                __reduce8(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            9 => {
                __reduce9(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            10 => {
                __reduce10(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            11 => {
                __reduce11(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            12 => {
                __reduce12(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            13 => {
                __reduce13(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            14 => {
                __reduce14(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            15 => {
                __reduce15(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            16 => {
                __reduce16(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            17 => {
                __reduce17(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            18 => {
                __reduce18(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            19 => {
                __reduce19(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            20 => {
                __reduce20(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            21 => {
                __reduce21(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            22 => {
                __reduce22(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            23 => {
                __reduce23(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            24 => {
                __reduce24(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            25 => {
                __reduce25(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            26 => {
                __reduce26(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            27 => {
                __reduce27(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            28 => {
                __reduce28(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            29 => {
                __reduce29(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            30 => {
                __reduce30(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            31 => {
                __reduce31(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            32 => {
                __reduce32(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            33 => {
                __reduce33(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            34 => {
                __reduce34(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            35 => {
                __reduce35(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            36 => {
                __reduce36(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            37 => {
                __reduce37(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            38 => {
                __reduce38(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            39 => {
                __reduce39(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            40 => {
                __reduce40(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            41 => {
                __reduce41(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            42 => {
                __reduce42(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            43 => {
                __reduce43(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            44 => {
                __reduce44(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            45 => {
                __reduce45(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            46 => {
                __reduce46(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            47 => {
                __reduce47(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            48 => {
                __reduce48(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            49 => {
                __reduce49(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            50 => {
                __reduce50(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            51 => {
                __reduce51(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            52 => {
                __reduce52(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            53 => {
                __reduce53(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            54 => {
                __reduce54(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            55 => {
                __reduce55(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            56 => {
                __reduce56(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            57 => {
                __reduce57(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            58 => {
                __reduce58(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            59 => {
                __reduce59(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            60 => {
                __reduce60(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            61 => {
                __reduce61(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            62 => {
                __reduce62(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            63 => {
                __reduce63(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            64 => {
                __reduce64(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            65 => {
                __reduce65(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            66 => {
                __reduce66(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            67 => {
                // __Patches = Patches => ActionFn(1);
                let __sym0 = __pop_Variant12(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action1::<>(ast, __sym0);
                return Some(Ok(__nt));
            }
            68 => {
                __reduce68(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            _ => panic!("invalid action code {__action}")
        };
        let __states_len = __states.len();
        __states.truncate(__states_len - __pop_states);
        let __state = *__states.last().unwrap();
        let __next_state = __goto(__state, __nonterminal);
        __states.push(__next_state);
        None
    }
    #[inline(never)]
    fn __symbol_type_mismatch() -> ! {
        panic!("symbol type mismatch")
    }
    fn __pop_Variant9<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, (BsnValue, BsnValue), usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant9(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant7<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnField, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant7(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant6<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnValue, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant6(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant12<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Entity, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant12(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant5<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Option<lexer::Token>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant5(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant0<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, String, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant0(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant10<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<(BsnValue, BsnValue)>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant10(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant8<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnField>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant8(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant11<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnValue>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant11(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant14<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant14(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant13<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, alloc::vec::Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant13(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant2<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, bool, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant2(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant1<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, f64, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant1(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant3<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, i128, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant3(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant4<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, lexer::Token, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant4(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __reduce0<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? = "," => ActionFn(55);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action55::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 0)
    }
    fn __reduce1<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? =  => ActionFn(56);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action56::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 0)
    }
    fn __reduce2<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? = "@" => ActionFn(51);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action51::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 1)
    }
    fn __reduce3<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? =  => ActionFn(52);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action52::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 1)
    }
    fn __reduce4<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Base = ":", "string_lit" => ActionFn(16);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action16::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 2)
    }
    fn __reduce5<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol => ActionFn(69);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action69::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce6<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol => ActionFn(70);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action70::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce7<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "{", Fields, "}" => ActionFn(71);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action71::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce8<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "{", Fields, "}" => ActionFn(72);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action72::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce9<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(73);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action73::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce10<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "(", NamedTupleArgs, ")" => ActionFn(74);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action74::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce11<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "string_lit" => ActionFn(35);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action35::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce12<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "float_lit" => ActionFn(36);
        let __sym0 = __pop_Variant1(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action36::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce13<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "bool_lit" => ActionFn(37);
        let __sym0 = __pop_Variant2(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action37::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce14<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "int_lit" => ActionFn(38);
        let __sym0 = __pop_Variant3(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action38::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce15<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, ",", "]" => ActionFn(57);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action57::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce16<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, "]" => ActionFn(58);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action58::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce17<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", "]" => ActionFn(40);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action40::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce18<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, ",", "]" => ActionFn(59);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action59::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce19<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, "]" => ActionFn(60);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action60::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce20<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", "]" => ActionFn(42);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action42::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce21<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Field = "ident", ":", Expr => ActionFn(27);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action27::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant7(__nt), __end));
        (3, 4)
    }
    fn __reduce22<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields =  => ActionFn(23);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action23::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (0, 5)
    }
    fn __reduce23<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner, "," => ActionFn(61);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action61::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (2, 5)
    }
    fn __reduce24<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner => ActionFn(62);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action62::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 5)
    }
    fn __reduce25<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = Field => ActionFn(25);
        let __sym0 = __pop_Variant7(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action25::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 6)
    }
    fn __reduce26<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = FieldsInner, ",", Field => ActionFn(26);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant7(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action26::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (3, 6)
    }
    fn __reduce27<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPair = "(", Expr, ",", Expr, ")" => ActionFn(45);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant6(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant6(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action45::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant9(__nt), __end));
        (5, 7)
    }
    fn __reduce28<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPair => ActionFn(43);
        let __sym0 = __pop_Variant9(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action43::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (1, 8)
    }
    fn __reduce29<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPairsInner, ",", MapPair => ActionFn(44);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant9(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant10(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action44::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (3, 8)
    }
    fn __reduce30<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "ident" => ActionFn(14);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action14::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce31<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "string_lit" => ActionFn(15);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action15::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce32<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs =  => ActionFn(28);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action28::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (0, 10)
    }
    fn __reduce33<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner, "," => ActionFn(63);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action63::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (2, 10)
    }
    fn __reduce34<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner => ActionFn(64);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action64::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 10)
    }
    fn __reduce35<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = Expr => ActionFn(30);
        let __sym0 = __pop_Variant6(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action30::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 11)
    }
    fn __reduce36<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = NamedTupleArgsInner, ",", Expr => ActionFn(31);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action31::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (3, 11)
    }
    fn __reduce37<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Name => ActionFn(8);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action8::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce38<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Base => ActionFn(9);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action9::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce39<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Relation => ActionFn(10);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action10::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce40<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol => ActionFn(75);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action75::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 12)
    }
    fn __reduce41<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol => ActionFn(76);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action76::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce42<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "{", Fields, "}" => ActionFn(77);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action77::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce43<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "{", Fields, "}" => ActionFn(78);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action78::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce44<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(79);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action79::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce45<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "(", NamedTupleArgs, ")" => ActionFn(80);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action80::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce46<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch => ActionFn(53);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action53::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (1, 13)
    }
    fn __reduce47<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch+, Patch => ActionFn(54);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant12(__symbols);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action54::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (2, 13)
    }
    fn __reduce48<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patches = Patch+ => ActionFn(7);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action7::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 14)
    }
    fn __reduce49<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relation = Symbol, "[", Relations, "]" => ActionFn(46);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant14(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action46::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (4, 15)
    }
    fn __reduce50<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations =  => ActionFn(47);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action47::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (0, 16)
    }
    fn __reduce51<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner, "," => ActionFn(65);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action65::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (2, 16)
    }
    fn __reduce52<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner => ActionFn(66);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action66::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 16)
    }
    fn __reduce53<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = RelationsInner, ",", Patches => ActionFn(49);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action49::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 17)
    }
    fn __reduce54<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = Patches => ActionFn(50);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action50::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 17)
    }
    fn __reduce55<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath => ActionFn(17);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action17::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 18)
    }
    fn __reduce56<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath, "<", SymbolArgs, ">" => ActionFn(18);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action18::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (4, 18)
    }
    fn __reduce57<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = Symbol => ActionFn(21);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action21::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 19)
    }
    fn __reduce58<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = SymbolArgs, ",", Symbol => ActionFn(22);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action22::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 19)
    }
    fn __reduce59<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = SymbolPath, "::", "ident" => ActionFn(19);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action19::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 20)
    }
    fn __reduce60<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = "ident" => ActionFn(20);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action20::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 20)
    }
    fn __reduce61<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = Patches => ActionFn(5);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action5::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 21)
    }
    fn __reduce62<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = TopLevelGroups, ",", Patches => ActionFn(6);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action6::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 21)
    }
    fn __reduce63<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches =  => ActionFn(3);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action3::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (0, 22)
    }
    fn __reduce64<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups, "," => ActionFn(67);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action67::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 22)
    }
    fn __reduce65<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups => ActionFn(68);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action68::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 22)
    }
    fn __reduce66<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __Patch = Patch => ActionFn(2);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action2::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 23)
    }
    fn __reduce68<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __TopLevelPatches = TopLevelPatches => ActionFn(0);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action0::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 25)
    }
}
#[allow(unused_imports)]
pub use self::__parse__Patches::PatchesParser;

#[rustfmt::skip]
#[allow(explicit_outlives_requirements, non_snake_case, non_camel_case_types, unused_mut, unused_variables, unused_imports, unused_parens, clippy::needless_lifetimes, clippy::type_complexity, clippy::needless_return, clippy::too_many_arguments, clippy::match_single_binding, clippy::clone_on_copy, clippy::unit_arg)]
mod __parse__TopLevelPatches {

    use crate::document::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst,
};
    use crate::parse::lexer;
    use bevy::ecs::entity::Entity;
    use std::cell::RefCell;
    #[allow(unused_extern_crates)]
    extern crate lalrpop_util as __lalrpop_util;
    #[allow(unused_imports)]
    use self::__lalrpop_util::state_machine as __state_machine;
    #[allow(unused_extern_crates)]
    extern crate alloc;
    use super::__ToTriple;
    #[allow(dead_code)]
    pub(crate) enum __Symbol<>
     {
        Variant0(String),
        Variant1(f64),
        Variant2(bool),
        Variant3(i128),
        Variant4(lexer::Token),
        Variant5(Option<lexer::Token>),
        Variant6(BsnValue),
        Variant7(BsnField),
        Variant8(Vec<BsnField>),
        Variant9((BsnValue, BsnValue)),
        Variant10(Vec<(BsnValue, BsnValue)>),
        Variant11(Vec<BsnValue>),
        Variant12(Entity),
        Variant13(alloc::vec::Vec<Entity>),
        Variant14(Vec<Entity>),
    }
    const __ACTION: &[i8] = &[
        // State 0
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 37, 36, 3, 0, 0,
        // State 1
        38, 0, 0, 0, 0, 0, -49, 0, 0, 0, 0, -49, 0, 0, 37, 36, 3, 0, 0,
        // State 2
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 3
        38, 53, 50, 49, 51, 10, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 4
        38, 0, 0, 0, 0, 0, -51, 0, 0, 0, 0, 0, 0, 0, 37, 36, 3, 0, 0,
        // State 5
        60, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 6
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 7
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 37, 36, 3, 0, 0,
        // State 8
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 9
        38, 53, 50, 49, 51, 10, 68, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 10
        38, 53, 50, 49, 51, 10, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 11
        60, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 12
        38, 53, 50, 49, 51, 10, 0, 0, -34, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 13
        38, 53, 50, 49, 51, 10, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 14
        60, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 15
        0, 0, 0, 0, 0, 0, 80, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 16
        38, 0, 0, 0, 0, 0, -52, 0, 0, 0, 0, 0, 0, 0, 37, 36, 3, 0, 0,
        // State 17
        60, 0, 0, 0, 0, 0, 0, 0, 0, 0, -24, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 18
        38, 53, 50, 49, 51, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 19
        38, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 20
        38, 53, 50, 49, 51, 10, 0, 0, -33, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 21
        60, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 22
        38, 53, 50, 49, 51, 10, 91, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 23
        38, 53, 50, 49, 51, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 24
        0, 0, 0, 0, 0, 0, 97, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 25
        38, 53, 50, 49, 51, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0, 52,
        // State 26
        -39, 0, 0, 0, 0, 0, -39, 0, 0, 0, 0, -39, 0, 0, -39, -39, -39, 0, 0,
        // State 27
        -38, 0, 0, 0, 0, 0, -38, 0, 0, 0, 0, -38, 0, 0, -38, -38, -38, 0, 0,
        // State 28
        -47, 0, 0, 0, 0, 0, -47, 0, 0, 0, 0, -47, 0, 0, -47, -47, -47, 0, 0,
        // State 29
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -62, 0, 0, 0, 0, 0, 0, 0,
        // State 30
        -40, 0, 0, 0, 0, 0, -40, 0, 0, 0, 0, -40, 0, 0, -40, -40, -40, 0, 0,
        // State 31
        -42, 0, 0, 0, 0, 5, -42, 4, 0, 6, 0, -42, 0, 0, -42, -42, -42, 0, 0,
        // State 32
        -56, 0, 0, 0, 0, -56, -56, -56, -56, -56, -56, -56, 7, -56, -56, -56, -56, 40, 0,
        // State 33
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0,
        // State 34
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 35
        41, 42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 36
        0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 37
        -61, 0, 0, 0, 0, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, -61, 0,
        // State 38
        -48, 0, 0, 0, 0, 0, -48, 0, 0, 0, 0, -48, 0, 0, -48, -48, -48, 0, 0,
        // State 39
        61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 40
        -31, 0, 0, 0, 0, 0, -31, 0, 0, 0, 0, -31, 0, 0, -31, -31, -31, 0, 0,
        // State 41
        -32, 0, 0, 0, 0, 0, -32, 0, 0, 0, 0, -32, 0, 0, -32, -32, -32, 0, 0,
        // State 42
        -5, 0, 0, 0, 0, 0, -5, 0, 0, 0, 0, -5, 0, 0, -5, -5, -5, 0, 0,
        // State 43
        -41, 0, 0, 0, 0, 0, -41, 11, 0, 12, 0, -41, 0, 0, -41, -41, -41, 0, 0,
        // State 44
        0, 0, 0, 0, 0, 0, -36, 0, -36, 0, 0, -36, 0, 0, 0, 0, 0, 0, 0,
        // State 45
        0, 0, 0, 0, 0, 0, 0, 0, 65, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 46
        0, 0, 0, 0, 0, 0, 0, 0, -35, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0,
        // State 47
        0, 0, 0, 0, 0, 0, -7, 14, -7, 15, -7, -7, 0, 0, 0, 0, 0, 0, 0,
        // State 48
        0, 0, 0, 0, 0, 0, -14, 0, -14, 0, -14, -14, 0, 0, 0, 0, 0, 0, 0,
        // State 49
        0, 0, 0, 0, 0, 0, -13, 0, -13, 0, -13, -13, 0, 0, 0, 0, 0, 0, 0,
        // State 50
        0, 0, 0, 0, 0, 0, -15, 0, -15, 0, -15, -15, 0, 0, 0, 0, 0, 0, 0,
        // State 51
        0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 52
        0, 0, 0, 0, 0, 0, -12, 0, -12, 0, -12, -12, 0, 0, 0, 0, 0, 0, 0,
        // State 53
        0, 0, 0, 0, 0, 0, -55, 0, 0, 0, 0, -55, 0, 0, 0, 0, 0, 0, 0,
        // State 54
        0, 0, 0, 0, 0, 0, 69, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 55
        0, 0, 0, 0, 0, 0, -53, 0, 0, 0, 0, 17, 0, 0, 0, 0, 0, 0, 0,
        // State 56
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -26, -26, 0, 0, 0, 0, 0, 0, 0,
        // State 57
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 70, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 58
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -25, 18, 0, 0, 0, 0, 0, 0, 0,
        // State 59
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 19, 0, 0, 0, 0,
        // State 60
        -60, 0, 0, 0, 0, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, -60, 0,
        // State 61
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -58, 0, -58, 0, 0, 0, 0, 0,
        // State 62
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 0, 71, 0, 0, 0, 0, 0,
        // State 63
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -63, 0, 0, 0, 0, 0, 0, 0,
        // State 64
        -46, 0, 0, 0, 0, 0, -46, 0, 0, 0, 0, -46, 0, 0, -46, -46, -46, 0, 0,
        // State 65
        0, 0, 0, 0, 0, 0, -6, 21, -6, 22, -6, -6, 0, 0, 0, 0, 0, 0, 0,
        // State 66
        0, 0, 0, 0, 0, 0, 77, 0, 0, 0, 0, 23, 0, 0, 0, 0, 0, 0, 0,
        // State 67
        0, 0, 0, 0, 0, 0, -18, 0, -18, 0, -18, -18, 0, 0, 0, 0, 0, 0, 0,
        // State 68
        -50, 0, 0, 0, 0, 0, -50, 0, 0, 0, 0, -50, 0, 0, -50, -50, -50, 0, 0,
        // State 69
        -44, 0, 0, 0, 0, 0, -44, 0, 0, 0, 0, -44, 0, 0, -44, -44, -44, 0, 0,
        // State 70
        -57, 0, 0, 0, 0, -57, -57, -57, -57, -57, -57, -57, 0, -57, -57, -57, -57, 0, 0,
        // State 71
        0, 0, 0, 0, 0, 0, 0, 0, 85, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 72
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 86, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 73
        0, 0, 0, 0, 0, 0, -37, 0, -37, 0, 0, -37, 0, 0, 0, 0, 0, 0, 0,
        // State 74
        0, 0, 0, 0, 0, 0, 0, 0, 87, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 75
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 88, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 76
        0, 0, 0, 0, 0, 0, -17, 0, -17, 0, -17, -17, 0, 0, 0, 0, 0, 0, 0,
        // State 77
        0, 0, 0, 0, 0, 0, -29, 0, 0, 0, 0, -29, 0, 0, 0, 0, 0, 0, 0,
        // State 78
        0, 0, 0, 0, 0, 0, 92, 0, 0, 0, 0, 25, 0, 0, 0, 0, 0, 0, 0,
        // State 79
        0, 0, 0, 0, 0, 0, -21, 0, -21, 0, -21, -21, 0, 0, 0, 0, 0, 0, 0,
        // State 80
        0, 0, 0, 0, 0, 0, -54, 0, 0, 0, 0, -54, 0, 0, 0, 0, 0, 0, 0,
        // State 81
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -27, -27, 0, 0, 0, 0, 0, 0, 0,
        // State 82
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -22, -22, 0, 0, 0, 0, 0, 0, 0,
        // State 83
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -59, 0, -59, 0, 0, 0, 0, 0,
        // State 84
        -45, 0, 0, 0, 0, 0, -45, 0, 0, 0, 0, -45, 0, 0, -45, -45, -45, 0, 0,
        // State 85
        -43, 0, 0, 0, 0, 0, -43, 0, 0, 0, 0, -43, 0, 0, -43, -43, -43, 0, 0,
        // State 86
        0, 0, 0, 0, 0, 0, -11, 0, -11, 0, -11, -11, 0, 0, 0, 0, 0, 0, 0,
        // State 87
        0, 0, 0, 0, 0, 0, -9, 0, -9, 0, -9, -9, 0, 0, 0, 0, 0, 0, 0,
        // State 88
        0, 0, 0, 0, 0, 0, 0, 0, 94, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 89
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 90
        0, 0, 0, 0, 0, 0, -16, 0, -16, 0, -16, -16, 0, 0, 0, 0, 0, 0, 0,
        // State 91
        0, 0, 0, 0, 0, 0, -20, 0, -20, 0, -20, -20, 0, 0, 0, 0, 0, 0, 0,
        // State 92
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0,
        // State 93
        0, 0, 0, 0, 0, 0, -10, 0, -10, 0, -10, -10, 0, 0, 0, 0, 0, 0, 0,
        // State 94
        0, 0, 0, 0, 0, 0, -8, 0, -8, 0, -8, -8, 0, 0, 0, 0, 0, 0, 0,
        // State 95
        0, 0, 0, 0, 0, 0, -30, 0, 0, 0, 0, -30, 0, 0, 0, 0, 0, 0, 0,
        // State 96
        0, 0, 0, 0, 0, 0, -19, 0, -19, 0, -19, -19, 0, 0, 0, 0, 0, 0, 0,
        // State 97
        0, 0, 0, 0, 0, 0, 0, 0, 99, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 98
        0, 0, 0, 0, 0, 0, -28, 0, 0, 0, 0, -28, 0, 0, 0, 0, 0, 0, 0,
    ];
    fn __action(state: i8, integer: usize) -> i8 {
        __ACTION[(state as usize) * 19 + integer]
    }
    const __EOF_ACTION: &[i8] = &[
        // State 0
        -64,
        // State 1
        -49,
        // State 2
        0,
        // State 3
        0,
        // State 4
        0,
        // State 5
        0,
        // State 6
        0,
        // State 7
        -65,
        // State 8
        0,
        // State 9
        0,
        // State 10
        0,
        // State 11
        0,
        // State 12
        0,
        // State 13
        0,
        // State 14
        0,
        // State 15
        0,
        // State 16
        0,
        // State 17
        0,
        // State 18
        0,
        // State 19
        0,
        // State 20
        0,
        // State 21
        0,
        // State 22
        0,
        // State 23
        0,
        // State 24
        0,
        // State 25
        0,
        // State 26
        -39,
        // State 27
        -38,
        // State 28
        -47,
        // State 29
        -62,
        // State 30
        -40,
        // State 31
        -42,
        // State 32
        -56,
        // State 33
        -66,
        // State 34
        -69,
        // State 35
        0,
        // State 36
        0,
        // State 37
        -61,
        // State 38
        -48,
        // State 39
        0,
        // State 40
        -31,
        // State 41
        -32,
        // State 42
        -5,
        // State 43
        -41,
        // State 44
        0,
        // State 45
        0,
        // State 46
        0,
        // State 47
        0,
        // State 48
        0,
        // State 49
        0,
        // State 50
        0,
        // State 51
        0,
        // State 52
        0,
        // State 53
        0,
        // State 54
        0,
        // State 55
        0,
        // State 56
        0,
        // State 57
        0,
        // State 58
        0,
        // State 59
        0,
        // State 60
        -60,
        // State 61
        0,
        // State 62
        0,
        // State 63
        -63,
        // State 64
        -46,
        // State 65
        0,
        // State 66
        0,
        // State 67
        0,
        // State 68
        -50,
        // State 69
        -44,
        // State 70
        -57,
        // State 71
        0,
        // State 72
        0,
        // State 73
        0,
        // State 74
        0,
        // State 75
        0,
        // State 76
        0,
        // State 77
        0,
        // State 78
        0,
        // State 79
        0,
        // State 80
        0,
        // State 81
        0,
        // State 82
        0,
        // State 83
        0,
        // State 84
        -45,
        // State 85
        -43,
        // State 86
        0,
        // State 87
        0,
        // State 88
        0,
        // State 89
        0,
        // State 90
        0,
        // State 91
        0,
        // State 92
        0,
        // State 93
        0,
        // State 94
        0,
        // State 95
        0,
        // State 96
        0,
        // State 97
        0,
        // State 98
        0,
    ];
    fn __goto(state: i8, nt: usize) -> i8 {
        match nt {
            2 => 26,
            3 => match state {
                12 | 22 => 73,
                18 => 82,
                23 => 92,
                25 => 97,
                _ => 44,
            },
            4 => match state {
                17 => 81,
                _ => 56,
            },
            5 => match state {
                11 => 72,
                14 => 75,
                21 => 89,
                _ => 57,
            },
            6 => 58,
            7 => match state {
                24 => 95,
                _ => 77,
            },
            8 => 78,
            9 => 27,
            10 => match state {
                10 => 71,
                13 => 74,
                20 => 88,
                _ => 45,
            },
            11 => match state {
                9 => 66,
                _ => 46,
            },
            12 => match state {
                1 => 38,
                _ => 28,
            },
            13 => 1,
            14 => match state {
                4 => 53,
                7 => 63,
                16 => 80,
                _ => 29,
            },
            15 => 30,
            16 => 54,
            17 => 55,
            18 => match state {
                0..=1 | 4 | 7 | 16 => 31,
                2 => 43,
                6 => 61,
                8 => 65,
                19 => 83,
                _ => 47,
            },
            19 => 62,
            20 => 32,
            21 => 33,
            22 => 34,
            _ => 0,
        }
    }
    #[allow(clippy::needless_raw_string_hashes)]
    const __TERMINAL: &[&str] = &[
        r###""ident""###,
        r###""string_lit""###,
        r###""float_lit""###,
        r###""bool_lit""###,
        r###""int_lit""###,
        r###""[""###,
        r###""]""###,
        r###""(""###,
        r###"")""###,
        r###""{""###,
        r###""}""###,
        r###"",""###,
        r###""<""###,
        r###"">""###,
        r###"":""###,
        r###""#""###,
        r###""@""###,
        r###""::""###,
        r###""map""###,
    ];
    fn __expected_tokens(__state: i8) -> alloc::vec::Vec<alloc::string::String> {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            let next_state = __action(__state, index);
            if next_state == 0 {
                None
            } else {
                Some(alloc::string::ToString::to_string(terminal))
            }
        }).collect()
    }
    fn __expected_tokens_from_states<
        '__0,
    >(
        __states: &[i8],
        _: core::marker::PhantomData<()>,
    ) -> alloc::vec::Vec<alloc::string::String>
    {
        __TERMINAL.iter().enumerate().filter_map(|(index, terminal)| {
            if __accepts(None, __states, Some(index), core::marker::PhantomData::<()>) {
                Some(alloc::string::ToString::to_string(terminal))
            } else {
                None
            }
        }).collect()
    }
    struct __StateMachine<'__0>
    where 
    {
        ast: &'__0 RefCell<SceneBsnAst>,
        __phantom: core::marker::PhantomData<()>,
    }
    impl<'__0> __state_machine::ParserDefinition for __StateMachine<'__0>
    where 
    {
        type Location = usize;
        type Error = lexer::Error;
        type Token = lexer::Token;
        type TokenIndex = usize;
        type Symbol = __Symbol<>;
        type Success = Entity;
        type StateIndex = i8;
        type Action = i8;
        type ReduceIndex = i8;
        type NonterminalIndex = usize;

        #[inline]
        fn start_location(&self) -> Self::Location {
              Default::default()
        }

        #[inline]
        fn start_state(&self) -> Self::StateIndex {
              0
        }

        #[inline]
        fn token_to_index(&self, token: &Self::Token) -> Option<usize> {
            __token_to_integer(token, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn action(&self, state: i8, integer: usize) -> i8 {
            __action(state, integer)
        }

        #[inline]
        fn error_action(&self, state: i8) -> i8 {
            __action(state, 19 - 1)
        }

        #[inline]
        fn eof_action(&self, state: i8) -> i8 {
            __EOF_ACTION[state as usize]
        }

        #[inline]
        fn goto(&self, state: i8, nt: usize) -> i8 {
            __goto(state, nt)
        }

        fn token_to_symbol(&self, token_index: usize, token: Self::Token) -> Self::Symbol {
            __token_to_symbol(token_index, token, core::marker::PhantomData::<()>)
        }

        fn expected_tokens(&self, state: i8) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens(state)
        }

        fn expected_tokens_from_states(&self, states: &[i8]) -> alloc::vec::Vec<alloc::string::String> {
            __expected_tokens_from_states(states, core::marker::PhantomData::<()>)
        }

        #[inline]
        fn uses_error_recovery(&self) -> bool {
            false
        }

        #[inline]
        fn error_recovery_symbol(
            &self,
            recovery: __state_machine::ErrorRecovery<Self>,
        ) -> Self::Symbol {
            panic!("error recovery not enabled for this grammar")
        }

        fn reduce(
            &mut self,
            action: i8,
            start_location: Option<&Self::Location>,
            states: &mut alloc::vec::Vec<i8>,
            symbols: &mut alloc::vec::Vec<__state_machine::SymbolTriple<Self>>,
        ) -> Option<__state_machine::ParseResult<Self>> {
            __reduce(
                self.ast,
                action,
                start_location,
                states,
                symbols,
                core::marker::PhantomData::<()>,
            )
        }

        fn simulate_reduce(&self, action: i8) -> __state_machine::SimulatedReduce<Self> {
            __simulate_reduce(action, core::marker::PhantomData::<()>)
        }
    }
    fn __token_to_integer<
    >(
        __token: &lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> Option<usize>
    {
        #[warn(unused_variables)]
        match __token {
            lexer::Token::Ident(_) if true => Some(0),
            lexer::Token::StringLit(_) if true => Some(1),
            lexer::Token::FloatLit(_) if true => Some(2),
            lexer::Token::BoolLit(_) if true => Some(3),
            lexer::Token::IntLit(_) if true => Some(4),
            lexer::Token::LBracket if true => Some(5),
            lexer::Token::RBracket if true => Some(6),
            lexer::Token::LParen if true => Some(7),
            lexer::Token::RParen if true => Some(8),
            lexer::Token::LBrace if true => Some(9),
            lexer::Token::RBrace if true => Some(10),
            lexer::Token::Comma if true => Some(11),
            lexer::Token::Less if true => Some(12),
            lexer::Token::Greater if true => Some(13),
            lexer::Token::Colon if true => Some(14),
            lexer::Token::Hash if true => Some(15),
            lexer::Token::At if true => Some(16),
            lexer::Token::DoubleColon if true => Some(17),
            lexer::Token::Map if true => Some(18),
            _ => None,
        }
    }
    fn __token_to_symbol<
    >(
        __token_index: usize,
        __token: lexer::Token,
        _: core::marker::PhantomData<()>,
    ) -> __Symbol<>
    {
        #[allow(clippy::manual_range_patterns)]match __token_index {
            0 | 1 => match __token {
                lexer::Token::Ident(__tok0) | lexer::Token::StringLit(__tok0) if true => __Symbol::Variant0(__tok0),
                _ => unreachable!(),
            },
            2 => match __token {
                lexer::Token::FloatLit(__tok0) if true => __Symbol::Variant1(__tok0),
                _ => unreachable!(),
            },
            3 => match __token {
                lexer::Token::BoolLit(__tok0) if true => __Symbol::Variant2(__tok0),
                _ => unreachable!(),
            },
            4 => match __token {
                lexer::Token::IntLit(__tok0) if true => __Symbol::Variant3(__tok0),
                _ => unreachable!(),
            },
            5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 => __Symbol::Variant4(__token),
            _ => unreachable!(),
        }
    }
    fn __simulate_reduce<
        '__0,
    >(
        __reduce_index: i8,
        _: core::marker::PhantomData<()>,
    ) -> __state_machine::SimulatedReduce<__StateMachine<'__0>>
    {
        match __reduce_index {
            0 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 0,
                }
            }
            1 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 0,
                }
            }
            2 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 1,
                }
            }
            3 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 1,
                }
            }
            4 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 2,
                }
            }
            5 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            6 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            7 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            8 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            9 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            10 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            11 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            12 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            13 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            14 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 3,
                }
            }
            15 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            16 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            17 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 3,
                }
            }
            18 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 3,
                }
            }
            19 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 3,
                }
            }
            20 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 3,
                }
            }
            21 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 4,
                }
            }
            22 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 5,
                }
            }
            23 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 5,
                }
            }
            24 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 5,
                }
            }
            25 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 6,
                }
            }
            26 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 6,
                }
            }
            27 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 7,
                }
            }
            28 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 8,
                }
            }
            29 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 8,
                }
            }
            30 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            31 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 9,
                }
            }
            32 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 10,
                }
            }
            33 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 10,
                }
            }
            34 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 10,
                }
            }
            35 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 11,
                }
            }
            36 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 11,
                }
            }
            37 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            38 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            39 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            40 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 12,
                }
            }
            41 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 12,
                }
            }
            42 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            43 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            44 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 5,
                    nonterminal_produced: 12,
                }
            }
            45 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 12,
                }
            }
            46 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 13,
                }
            }
            47 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 13,
                }
            }
            48 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 14,
                }
            }
            49 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 15,
                }
            }
            50 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 16,
                }
            }
            51 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 16,
                }
            }
            52 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 16,
                }
            }
            53 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 17,
                }
            }
            54 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 17,
                }
            }
            55 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 18,
                }
            }
            56 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 4,
                    nonterminal_produced: 18,
                }
            }
            57 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 19,
                }
            }
            58 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 19,
                }
            }
            59 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 20,
                }
            }
            60 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 20,
                }
            }
            61 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 21,
                }
            }
            62 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 3,
                    nonterminal_produced: 21,
                }
            }
            63 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 0,
                    nonterminal_produced: 22,
                }
            }
            64 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 2,
                    nonterminal_produced: 22,
                }
            }
            65 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 22,
                }
            }
            66 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 23,
                }
            }
            67 => {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop: 1,
                    nonterminal_produced: 24,
                }
            }
            68 => __state_machine::SimulatedReduce::Accept,
            _ => panic!("invalid reduction index {__reduce_index}")
        }
    }
    pub struct TopLevelPatchesParser {
        _priv: (),
    }

    impl Default for TopLevelPatchesParser { fn default() -> Self { Self::new() } }
    impl TopLevelPatchesParser {
        pub fn new() -> TopLevelPatchesParser {
            TopLevelPatchesParser {
                _priv: (),
            }
        }

        #[allow(dead_code)]
        pub fn parse<
            __TOKEN: __ToTriple<>,
            __TOKENS: IntoIterator<Item=__TOKEN>,
        >(
            &self,
            ast: &RefCell<SceneBsnAst>,
            __tokens0: __TOKENS,
        ) -> Result<Entity, __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>
        {
            let __tokens = __tokens0.into_iter();
            let mut __tokens = __tokens.map(|t| __ToTriple::to_triple(t));
            __state_machine::Parser::drive(
                __StateMachine {
                    ast,
                    __phantom: core::marker::PhantomData::<()>,
                },
                __tokens,
            )
        }
    }
    fn __accepts<
        '__0,
    >(
        __error_state: Option<i8>,
        __states: &[i8],
        __opt_integer: Option<usize>,
        _: core::marker::PhantomData<()>,
    ) -> bool
    {
        let mut __states = __states.to_vec();
        __states.extend(__error_state);
        loop {
            let mut __states_len = __states.len();
            let __top = __states[__states_len - 1];
            let __action = match __opt_integer {
                None => __EOF_ACTION[__top as usize],
                Some(__integer) => __action(__top, __integer),
            };
            if __action == 0 { return false; }
            if __action > 0 { return true; }
            let (__to_pop, __nt) = match __simulate_reduce(-(__action + 1), core::marker::PhantomData::<()>) {
                __state_machine::SimulatedReduce::Reduce {
                    states_to_pop, nonterminal_produced
                } => (states_to_pop, nonterminal_produced),
                __state_machine::SimulatedReduce::Accept => return true,
            };
            __states_len -= __to_pop;
            __states.truncate(__states_len);
            let __top = __states[__states_len - 1];
            let __next_state = __goto(__top, __nt);
            __states.push(__next_state);
        }
    }
    fn __reduce<
    >(
        ast: &RefCell<SceneBsnAst>,
        __action: i8,
        __lookahead_start: Option<&usize>,
        __states: &mut alloc::vec::Vec<i8>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> Option<Result<Entity,__lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>>
    {
        let (__pop_states, __nonterminal) = match __action {
            0 => {
                __reduce0(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            1 => {
                __reduce1(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            2 => {
                __reduce2(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            3 => {
                __reduce3(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            4 => {
                __reduce4(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            5 => {
                __reduce5(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            6 => {
                __reduce6(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            7 => {
                __reduce7(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            8 => {
                __reduce8(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            9 => {
                __reduce9(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            10 => {
                __reduce10(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            11 => {
                __reduce11(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            12 => {
                __reduce12(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            13 => {
                __reduce13(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            14 => {
                __reduce14(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            15 => {
                __reduce15(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            16 => {
                __reduce16(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            17 => {
                __reduce17(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            18 => {
                __reduce18(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            19 => {
                __reduce19(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            20 => {
                __reduce20(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            21 => {
                __reduce21(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            22 => {
                __reduce22(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            23 => {
                __reduce23(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            24 => {
                __reduce24(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            25 => {
                __reduce25(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            26 => {
                __reduce26(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            27 => {
                __reduce27(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            28 => {
                __reduce28(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            29 => {
                __reduce29(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            30 => {
                __reduce30(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            31 => {
                __reduce31(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            32 => {
                __reduce32(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            33 => {
                __reduce33(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            34 => {
                __reduce34(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            35 => {
                __reduce35(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            36 => {
                __reduce36(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            37 => {
                __reduce37(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            38 => {
                __reduce38(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            39 => {
                __reduce39(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            40 => {
                __reduce40(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            41 => {
                __reduce41(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            42 => {
                __reduce42(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            43 => {
                __reduce43(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            44 => {
                __reduce44(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            45 => {
                __reduce45(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            46 => {
                __reduce46(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            47 => {
                __reduce47(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            48 => {
                __reduce48(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            49 => {
                __reduce49(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            50 => {
                __reduce50(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            51 => {
                __reduce51(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            52 => {
                __reduce52(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            53 => {
                __reduce53(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            54 => {
                __reduce54(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            55 => {
                __reduce55(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            56 => {
                __reduce56(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            57 => {
                __reduce57(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            58 => {
                __reduce58(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            59 => {
                __reduce59(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            60 => {
                __reduce60(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            61 => {
                __reduce61(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            62 => {
                __reduce62(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            63 => {
                __reduce63(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            64 => {
                __reduce64(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            65 => {
                __reduce65(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            66 => {
                __reduce66(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            67 => {
                __reduce67(ast, __lookahead_start, __symbols, core::marker::PhantomData::<()>)
            }
            68 => {
                // __TopLevelPatches = TopLevelPatches => ActionFn(0);
                let __sym0 = __pop_Variant12(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action0::<>(ast, __sym0);
                return Some(Ok(__nt));
            }
            _ => panic!("invalid action code {__action}")
        };
        let __states_len = __states.len();
        __states.truncate(__states_len - __pop_states);
        let __state = *__states.last().unwrap();
        let __next_state = __goto(__state, __nonterminal);
        __states.push(__next_state);
        None
    }
    #[inline(never)]
    fn __symbol_type_mismatch() -> ! {
        panic!("symbol type mismatch")
    }
    fn __pop_Variant9<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, (BsnValue, BsnValue), usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant9(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant7<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnField, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant7(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant6<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, BsnValue, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant6(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant12<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Entity, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant12(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant5<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Option<lexer::Token>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant5(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant0<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, String, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant0(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant10<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<(BsnValue, BsnValue)>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant10(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant8<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnField>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant8(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant11<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<BsnValue>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant11(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant14<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant14(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant13<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, alloc::vec::Vec<Entity>, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant13(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant2<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, bool, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant2(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant1<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, f64, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant1(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant3<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, i128, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant3(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __pop_Variant4<
    >(
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>
    ) -> (usize, lexer::Token, usize)
     {
        match __symbols.pop() {
            Some((__l, __Symbol::Variant4(__v), __r)) => (__l, __v, __r),
            _ => __symbol_type_mismatch()
        }
    }
    fn __reduce0<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? = "," => ActionFn(55);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action55::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 0)
    }
    fn __reduce1<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // ","? =  => ActionFn(56);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action56::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 0)
    }
    fn __reduce2<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? = "@" => ActionFn(51);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action51::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (1, 1)
    }
    fn __reduce3<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // "@"? =  => ActionFn(52);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action52::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant5(__nt), __end));
        (0, 1)
    }
    fn __reduce4<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Base = ":", "string_lit" => ActionFn(16);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action16::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 2)
    }
    fn __reduce5<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol => ActionFn(69);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action69::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce6<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol => ActionFn(70);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action70::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce7<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "{", Fields, "}" => ActionFn(71);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action71::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce8<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "{", Fields, "}" => ActionFn(72);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action72::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce9<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(73);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action73::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce10<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = Symbol, "(", NamedTupleArgs, ")" => ActionFn(74);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action74::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce11<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "string_lit" => ActionFn(35);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action35::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce12<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "float_lit" => ActionFn(36);
        let __sym0 = __pop_Variant1(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action36::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce13<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "bool_lit" => ActionFn(37);
        let __sym0 = __pop_Variant2(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action37::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce14<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "int_lit" => ActionFn(38);
        let __sym0 = __pop_Variant3(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action38::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (1, 3)
    }
    fn __reduce15<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, ",", "]" => ActionFn(57);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action57::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce16<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", NamedTupleArgsInner, "]" => ActionFn(58);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant11(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action58::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce17<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "[", "]" => ActionFn(40);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action40::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (2, 3)
    }
    fn __reduce18<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, ",", "]" => ActionFn(59);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action59::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (5, 3)
    }
    fn __reduce19<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", MapPairsInner, "]" => ActionFn(60);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant10(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action60::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (4, 3)
    }
    fn __reduce20<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Expr = "map", "[", "]" => ActionFn(42);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action42::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant6(__nt), __end));
        (3, 3)
    }
    fn __reduce21<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Field = "ident", ":", Expr => ActionFn(27);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action27::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant7(__nt), __end));
        (3, 4)
    }
    fn __reduce22<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields =  => ActionFn(23);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action23::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (0, 5)
    }
    fn __reduce23<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner, "," => ActionFn(61);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action61::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (2, 5)
    }
    fn __reduce24<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Fields = FieldsInner => ActionFn(62);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action62::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 5)
    }
    fn __reduce25<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = Field => ActionFn(25);
        let __sym0 = __pop_Variant7(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action25::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (1, 6)
    }
    fn __reduce26<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // FieldsInner = FieldsInner, ",", Field => ActionFn(26);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant7(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant8(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action26::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant8(__nt), __end));
        (3, 6)
    }
    fn __reduce27<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPair = "(", Expr, ",", Expr, ")" => ActionFn(45);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant6(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant6(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action45::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant9(__nt), __end));
        (5, 7)
    }
    fn __reduce28<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPair => ActionFn(43);
        let __sym0 = __pop_Variant9(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action43::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (1, 8)
    }
    fn __reduce29<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // MapPairsInner = MapPairsInner, ",", MapPair => ActionFn(44);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant9(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant10(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action44::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant10(__nt), __end));
        (3, 8)
    }
    fn __reduce30<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "ident" => ActionFn(14);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action14::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce31<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Name = "#", "string_lit" => ActionFn(15);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action15::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (2, 9)
    }
    fn __reduce32<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs =  => ActionFn(28);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action28::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (0, 10)
    }
    fn __reduce33<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner, "," => ActionFn(63);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action63::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (2, 10)
    }
    fn __reduce34<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgs = NamedTupleArgsInner => ActionFn(64);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action64::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 10)
    }
    fn __reduce35<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = Expr => ActionFn(30);
        let __sym0 = __pop_Variant6(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action30::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (1, 11)
    }
    fn __reduce36<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // NamedTupleArgsInner = NamedTupleArgsInner, ",", Expr => ActionFn(31);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant6(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant11(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action31::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant11(__nt), __end));
        (3, 11)
    }
    fn __reduce37<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Name => ActionFn(8);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action8::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce38<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Base => ActionFn(9);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action9::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce39<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Relation => ActionFn(10);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action10::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce40<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol => ActionFn(75);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action75::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 12)
    }
    fn __reduce41<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol => ActionFn(76);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action76::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 12)
    }
    fn __reduce42<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "{", Fields, "}" => ActionFn(77);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant8(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action77::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce43<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "{", Fields, "}" => ActionFn(78);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant8(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action78::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce44<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = "@", Symbol, "(", NamedTupleArgs, ")" => ActionFn(79);
        assert!(__symbols.len() >= 5);
        let __sym4 = __pop_Variant4(__symbols);
        let __sym3 = __pop_Variant11(__symbols);
        let __sym2 = __pop_Variant4(__symbols);
        let __sym1 = __pop_Variant0(__symbols);
        let __sym0 = __pop_Variant4(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym4.2.clone();
        let __nt = super::__action79::<>(ast, __sym0, __sym1, __sym2, __sym3, __sym4);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (5, 12)
    }
    fn __reduce45<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch = Symbol, "(", NamedTupleArgs, ")" => ActionFn(80);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant11(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action80::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (4, 12)
    }
    fn __reduce46<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch => ActionFn(53);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action53::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (1, 13)
    }
    fn __reduce47<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patch+ = Patch+, Patch => ActionFn(54);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant12(__symbols);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action54::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant13(__nt), __end));
        (2, 13)
    }
    fn __reduce48<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Patches = Patch+ => ActionFn(7);
        let __sym0 = __pop_Variant13(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action7::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 14)
    }
    fn __reduce49<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relation = Symbol, "[", Relations, "]" => ActionFn(46);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant14(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action46::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (4, 15)
    }
    fn __reduce50<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations =  => ActionFn(47);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action47::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (0, 16)
    }
    fn __reduce51<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner, "," => ActionFn(65);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action65::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (2, 16)
    }
    fn __reduce52<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Relations = RelationsInner => ActionFn(66);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action66::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 16)
    }
    fn __reduce53<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = RelationsInner, ",", Patches => ActionFn(49);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action49::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 17)
    }
    fn __reduce54<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // RelationsInner = Patches => ActionFn(50);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action50::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 17)
    }
    fn __reduce55<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath => ActionFn(17);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action17::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 18)
    }
    fn __reduce56<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // Symbol = SymbolPath, "<", SymbolArgs, ">" => ActionFn(18);
        assert!(__symbols.len() >= 4);
        let __sym3 = __pop_Variant4(__symbols);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym3.2.clone();
        let __nt = super::__action18::<>(ast, __sym0, __sym1, __sym2, __sym3);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (4, 18)
    }
    fn __reduce57<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = Symbol => ActionFn(21);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action21::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 19)
    }
    fn __reduce58<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolArgs = SymbolArgs, ",", Symbol => ActionFn(22);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action22::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 19)
    }
    fn __reduce59<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = SymbolPath, "::", "ident" => ActionFn(19);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant0(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action19::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (3, 20)
    }
    fn __reduce60<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // SymbolPath = "ident" => ActionFn(20);
        let __sym0 = __pop_Variant0(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action20::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant0(__nt), __end));
        (1, 20)
    }
    fn __reduce61<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = Patches => ActionFn(5);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action5::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (1, 21)
    }
    fn __reduce62<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelGroups = TopLevelGroups, ",", Patches => ActionFn(6);
        assert!(__symbols.len() >= 3);
        let __sym2 = __pop_Variant12(__symbols);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym2.2.clone();
        let __nt = super::__action6::<>(ast, __sym0, __sym1, __sym2);
        __symbols.push((__start, __Symbol::Variant14(__nt), __end));
        (3, 21)
    }
    fn __reduce63<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches =  => ActionFn(3);
        let __start = __lookahead_start.cloned().or_else(|| __symbols.last().map(|s| s.2)).unwrap_or_default();
        let __end = __start;
        let __nt = super::__action3::<>(ast, &__start, &__end);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (0, 22)
    }
    fn __reduce64<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups, "," => ActionFn(67);
        assert!(__symbols.len() >= 2);
        let __sym1 = __pop_Variant4(__symbols);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym1.2.clone();
        let __nt = super::__action67::<>(ast, __sym0, __sym1);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (2, 22)
    }
    fn __reduce65<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // TopLevelPatches = TopLevelGroups => ActionFn(68);
        let __sym0 = __pop_Variant14(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action68::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 22)
    }
    fn __reduce66<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __Patch = Patch => ActionFn(2);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action2::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 23)
    }
    fn __reduce67<
    >(
        ast: &RefCell<SceneBsnAst>,
        __lookahead_start: Option<&usize>,
        __symbols: &mut alloc::vec::Vec<(usize,__Symbol<>,usize)>,
        _: core::marker::PhantomData<()>,
    ) -> (usize, usize)
    {
        // __Patches = Patches => ActionFn(1);
        let __sym0 = __pop_Variant12(__symbols);
        let __start = __sym0.0.clone();
        let __end = __sym0.2.clone();
        let __nt = super::__action1::<>(ast, __sym0);
        __symbols.push((__start, __Symbol::Variant12(__nt), __end));
        (1, 24)
    }
}
#[allow(unused_imports)]
pub use self::__parse__TopLevelPatches::TopLevelPatchesParser;

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action0<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, Entity, usize),
) -> Entity
{
    __0
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action1<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, Entity, usize),
) -> Entity
{
    __0
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action2<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, Entity, usize),
) -> Entity
{
    __0
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action3<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Entity
{
    ast.borrow_mut().world.spawn(BsnPatches(Vec::new())).id()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action4<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, g, _): (usize, Vec<Entity>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
) -> Entity
{
    {
        if g.len() == 1 {
            g[0]
        } else {
            let mut ast = ast.borrow_mut();
            let children = ast.world.spawn(BsnPatch::Children(g)).id();
            ast.world.spawn(BsnPatches(vec![children])).id()
        }
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action5<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, t, _): (usize, Entity, usize),
) -> Vec<Entity>
{
    { let mut v = Vec::new(); v.push(t); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action6<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, Vec<Entity>, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, Entity, usize),
) -> Vec<Entity>
{
    { let mut h = h; h.push(t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action7<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, p, _): (usize, alloc::vec::Vec<Entity>, usize),
) -> Entity
{
    ast.borrow_mut().world.spawn(BsnPatches(p)).id()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action8<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, n, _): (usize, String, usize),
) -> Entity
{
    ast.borrow_mut().world.spawn(BsnPatch::Name(n)).id()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action9<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, b, _): (usize, String, usize),
) -> Entity
{
    ast.borrow_mut().world.spawn(BsnPatch::Base(b)).id()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action10<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, r, _): (usize, Vec<Entity>, usize),
) -> Entity
{
    ast.borrow_mut().world.spawn(BsnPatch::Children(r)).id()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action11<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
) -> Entity
{
    {
        let patch = if a.is_some() {
            BsnPatch::Template(s, None)
        } else {
            BsnPatch::Type(s)
        };
        ast.borrow_mut().world.spawn(patch).id()
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action12<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, f, _): (usize, Vec<BsnField>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> Entity
{
    {
        let patch = if a.is_some() {
            BsnPatch::Template(s, Some(BsnStructFields(f)))
        } else {
            BsnPatch::Struct(BsnStructData {
                type_path: s,
                fields: BsnStructFields(f),
            })
        };
        ast.borrow_mut().world.spawn(patch).id()
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action13<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, n, _): (usize, Vec<BsnValue>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> Entity
{
    {
        ast.borrow_mut().world.spawn(BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: s,
            values: n,
        })).id()
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action14<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, i, _): (usize, String, usize),
) -> String
{
    i
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action15<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, s, _): (usize, String, usize),
) -> String
{
    s
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action16<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, s, _): (usize, String, usize),
) -> String
{
    s
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action17<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
) -> String
{
    s
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action18<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, a, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> String
{
    {
        let mut s = s;
        s.push('<');
        s.push_str(&a);
        s.push('>');
        s
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action19<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, n, _): (usize, String, usize),
) -> String
{
    { let mut s = s; s.push_str("::"); s.push_str(&n); s }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action20<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, n, _): (usize, String, usize),
) -> String
{
    n
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action21<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
) -> String
{
    s
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action22<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, String, usize),
) -> String
{
    { let mut h = h; h.push_str(", "); h.push_str(&t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action23<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Vec<BsnField>
{
    Vec::new()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action24<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, f, _): (usize, Vec<BsnField>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
) -> Vec<BsnField>
{
    f
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action25<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, t, _): (usize, BsnField, usize),
) -> Vec<BsnField>
{
    { let mut v = Vec::new(); v.push(t); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action26<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, Vec<BsnField>, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, BsnField, usize),
) -> Vec<BsnField>
{
    { let mut h = h; h.push(t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action27<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, n, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, e, _): (usize, BsnValue, usize),
) -> BsnField
{
    BsnField { name: n, value: e }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action28<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Vec<BsnValue>
{
    Vec::new()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action29<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, n, _): (usize, Vec<BsnValue>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
) -> Vec<BsnValue>
{
    n
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action30<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, t, _): (usize, BsnValue, usize),
) -> Vec<BsnValue>
{
    { let mut v = Vec::new(); v.push(t); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action31<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, Vec<BsnValue>, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, BsnValue, usize),
) -> Vec<BsnValue>
{
    { let mut h = h; h.push(t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action32<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
) -> BsnValue
{
    BsnValue::Type(s)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action33<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, f, _): (usize, Vec<BsnField>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    {
        BsnValue::Struct(BsnStructData {
            type_path: s,
            fields: BsnStructFields(f),
        })
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action34<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, a, _): (usize, Option<lexer::Token>, usize),
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, n, _): (usize, Vec<BsnValue>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    {
        BsnValue::TupleStruct(BsnTupleStructData {
            type_path: s,
            values: n,
        })
    }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action35<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
) -> BsnValue
{
    BsnValue::String(s)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action36<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, f, _): (usize, f64, usize),
) -> BsnValue
{
    BsnValue::Float(f)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action37<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, b, _): (usize, bool, usize),
) -> BsnValue
{
    BsnValue::Bool(b)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action38<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, i, _): (usize, i128, usize),
) -> BsnValue
{
    BsnValue::Int(i)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action39<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, n, _): (usize, Vec<BsnValue>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    BsnValue::List(n)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action40<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, lexer::Token, usize),
    (_, __1, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    BsnValue::List(Vec::new())
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action41<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, p, _): (usize, Vec<(BsnValue, BsnValue)>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    BsnValue::Map(p)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action42<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, lexer::Token, usize),
    (_, __1, _): (usize, lexer::Token, usize),
    (_, __2, _): (usize, lexer::Token, usize),
) -> BsnValue
{
    BsnValue::Map(Vec::new())
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action43<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, t, _): (usize, (BsnValue, BsnValue), usize),
) -> Vec<(BsnValue, BsnValue)>
{
    { let mut v = Vec::new(); v.push(t); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action44<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, Vec<(BsnValue, BsnValue)>, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, (BsnValue, BsnValue), usize),
) -> Vec<(BsnValue, BsnValue)>
{
    { let mut h = h; h.push(t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action45<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, _, _): (usize, lexer::Token, usize),
    (_, k, _): (usize, BsnValue, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, v, _): (usize, BsnValue, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> (BsnValue, BsnValue)
{
    (k, v)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action46<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, s, _): (usize, String, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, r, _): (usize, Vec<Entity>, usize),
    (_, _, _): (usize, lexer::Token, usize),
) -> Vec<Entity>
{
    r
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action47<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Vec<Entity>
{
    Vec::new()
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action48<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, r, _): (usize, Vec<Entity>, usize),
    (_, _, _): (usize, Option<lexer::Token>, usize),
) -> Vec<Entity>
{
    r
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action49<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, h, _): (usize, Vec<Entity>, usize),
    (_, _, _): (usize, lexer::Token, usize),
    (_, t, _): (usize, Entity, usize),
) -> Vec<Entity>
{
    { let mut h = h; h.push(t); h }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action50<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, t, _): (usize, Entity, usize),
) -> Vec<Entity>
{
    { let mut v = Vec::new(); v.push(t); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action51<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, lexer::Token, usize),
) -> Option<lexer::Token>
{
    Some(__0)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action52<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Option<lexer::Token>
{
    None
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action53<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, Entity, usize),
) -> alloc::vec::Vec<Entity>
{
    alloc::vec![__0]
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action54<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, v, _): (usize, alloc::vec::Vec<Entity>, usize),
    (_, e, _): (usize, Entity, usize),
) -> alloc::vec::Vec<Entity>
{
    { let mut v = v; v.push(e); v }
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action55<
>(
    ast: &RefCell<SceneBsnAst>,
    (_, __0, _): (usize, lexer::Token, usize),
) -> Option<lexer::Token>
{
    Some(__0)
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes, clippy::just_underscores_and_digits)]
fn __action56<
>(
    ast: &RefCell<SceneBsnAst>,
    __lookbehind: &usize,
    __lookahead: &usize,
) -> Option<lexer::Token>
{
    None
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action57<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, Vec<BsnValue>, usize),
    __2: (usize, lexer::Token, usize),
    __3: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __2.0;
    let __end0 = __2.2;
    let __temp0 = __action55(
        ast,
        __2,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action39(
        ast,
        __0,
        __1,
        __temp0,
        __3,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action58<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, Vec<BsnValue>, usize),
    __2: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __1.2;
    let __end0 = __2.0;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action39(
        ast,
        __0,
        __1,
        __temp0,
        __2,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action59<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<(BsnValue, BsnValue)>, usize),
    __3: (usize, lexer::Token, usize),
    __4: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __3.0;
    let __end0 = __3.2;
    let __temp0 = __action55(
        ast,
        __3,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action41(
        ast,
        __0,
        __1,
        __2,
        __temp0,
        __4,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action60<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<(BsnValue, BsnValue)>, usize),
    __3: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __2.2;
    let __end0 = __3.0;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action41(
        ast,
        __0,
        __1,
        __2,
        __temp0,
        __3,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action61<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<BsnField>, usize),
    __1: (usize, lexer::Token, usize),
) -> Vec<BsnField>
{
    let __start0 = __1.0;
    let __end0 = __1.2;
    let __temp0 = __action55(
        ast,
        __1,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action24(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action62<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<BsnField>, usize),
) -> Vec<BsnField>
{
    let __start0 = __0.2;
    let __end0 = __0.2;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action24(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action63<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<BsnValue>, usize),
    __1: (usize, lexer::Token, usize),
) -> Vec<BsnValue>
{
    let __start0 = __1.0;
    let __end0 = __1.2;
    let __temp0 = __action55(
        ast,
        __1,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action29(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action64<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<BsnValue>, usize),
) -> Vec<BsnValue>
{
    let __start0 = __0.2;
    let __end0 = __0.2;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action29(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action65<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<Entity>, usize),
    __1: (usize, lexer::Token, usize),
) -> Vec<Entity>
{
    let __start0 = __1.0;
    let __end0 = __1.2;
    let __temp0 = __action55(
        ast,
        __1,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action48(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action66<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<Entity>, usize),
) -> Vec<Entity>
{
    let __start0 = __0.2;
    let __end0 = __0.2;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action48(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action67<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<Entity>, usize),
    __1: (usize, lexer::Token, usize),
) -> Entity
{
    let __start0 = __1.0;
    let __end0 = __1.2;
    let __temp0 = __action55(
        ast,
        __1,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action4(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action68<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, Vec<Entity>, usize),
) -> Entity
{
    let __start0 = __0.2;
    let __end0 = __0.2;
    let __temp0 = __action56(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action4(
        ast,
        __0,
        __temp0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action69<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action32(
        ast,
        __temp0,
        __1,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action70<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action32(
        ast,
        __temp0,
        __0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action71<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
    __2: (usize, lexer::Token, usize),
    __3: (usize, Vec<BsnField>, usize),
    __4: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action33(
        ast,
        __temp0,
        __1,
        __2,
        __3,
        __4,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action72<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<BsnField>, usize),
    __3: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action33(
        ast,
        __temp0,
        __0,
        __1,
        __2,
        __3,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action73<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
    __2: (usize, lexer::Token, usize),
    __3: (usize, Vec<BsnValue>, usize),
    __4: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action34(
        ast,
        __temp0,
        __1,
        __2,
        __3,
        __4,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action74<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<BsnValue>, usize),
    __3: (usize, lexer::Token, usize),
) -> BsnValue
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action34(
        ast,
        __temp0,
        __0,
        __1,
        __2,
        __3,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action75<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action11(
        ast,
        __temp0,
        __1,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action76<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action11(
        ast,
        __temp0,
        __0,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action77<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
    __2: (usize, lexer::Token, usize),
    __3: (usize, Vec<BsnField>, usize),
    __4: (usize, lexer::Token, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action12(
        ast,
        __temp0,
        __1,
        __2,
        __3,
        __4,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action78<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<BsnField>, usize),
    __3: (usize, lexer::Token, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action12(
        ast,
        __temp0,
        __0,
        __1,
        __2,
        __3,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action79<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, lexer::Token, usize),
    __1: (usize, String, usize),
    __2: (usize, lexer::Token, usize),
    __3: (usize, Vec<BsnValue>, usize),
    __4: (usize, lexer::Token, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.2;
    let __temp0 = __action51(
        ast,
        __0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action13(
        ast,
        __temp0,
        __1,
        __2,
        __3,
        __4,
    )
}

#[allow(unused_variables)]
#[allow(clippy::too_many_arguments, clippy::needless_lifetimes,
    clippy::just_underscores_and_digits)]
fn __action80<
>(
    ast: &RefCell<SceneBsnAst>,
    __0: (usize, String, usize),
    __1: (usize, lexer::Token, usize),
    __2: (usize, Vec<BsnValue>, usize),
    __3: (usize, lexer::Token, usize),
) -> Entity
{
    let __start0 = __0.0;
    let __end0 = __0.0;
    let __temp0 = __action52(
        ast,
        &__start0,
        &__end0,
    );
    let __temp0 = (__start0, __temp0, __end0);
    __action13(
        ast,
        __temp0,
        __0,
        __1,
        __2,
        __3,
    )
}

#[allow(clippy::type_complexity, dead_code)]
pub trait __ToTriple<>
{
    fn to_triple(self) -> Result<(usize,lexer::Token,usize), __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>>;
}

impl<> __ToTriple<> for (usize, lexer::Token, usize)
{
    fn to_triple(self) -> Result<(usize,lexer::Token,usize), __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>> {
        Ok(self)
    }
}
impl<> __ToTriple<> for Result<(usize, lexer::Token, usize), lexer::Error>
{
    fn to_triple(self) -> Result<(usize,lexer::Token,usize), __lalrpop_util::ParseError<usize, lexer::Token, lexer::Error>> {
        self.map_err(|error| __lalrpop_util::ParseError::User { error })
    }
}
