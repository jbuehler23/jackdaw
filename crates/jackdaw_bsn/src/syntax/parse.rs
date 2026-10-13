use crate::syntax::{
    lex::Token,
    parse_stream::{Parse, ParseError, ParseStream},
    types::{
        At, Bool, Colon, Comma, DoubleColon, DoubleMinus, Float, Greater, Hash, Ident, Int, LBrace,
        LBracket, LParen, Less, RBrace, RBracket, RParen, StringLit,
    },
    types::{
        Bsn, BsnEntry, BsnFields, BsnListRoot, BsnNamedField, BsnRoot, BsnScene, BsnSceneList,
        BsnSceneListItems, BsnType, BsnValue, Path, PathSegment,
    },
};

impl Parse for BsnRoot {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        Ok(BsnRoot(input.parse::<Bsn>()?))
    }
}

impl Parse for BsnListRoot {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let items = input.parse::<BsnSceneListItems>()?;
        if !input.is_empty() {
            return Err(match input.next_token()? {
                Some(token) => ParseError::UnexpectedToken(token.into()),
                None => ParseError::EndOfInput,
            });
        }
        Ok(BsnListRoot(items))
    }
}

impl Parse for Bsn {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let mut entries = Vec::new();
        while !(input.is_empty_or_closing_delimiter() || input.peek::<DoubleMinus>()) {
            entries.push(input.parse::<BsnEntry>()?);
        }
        Ok(Bsn { entries })
    }
}

impl Parse for BsnEntry {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        Ok(if input.peek::<Hash>() {
            let _ = input.parse::<Hash>()?;
            if input.peek::<StringLit>() {
                BsnEntry::Name(Ident(input.parse::<StringLit>()?.0))
            } else {
                BsnEntry::Name(input.parse::<Ident>()?)
            }
        } else if input.peek::<Colon>() {
            let _ = input.parse::<Colon>()?;
            let path = input.parse::<StringLit>()?;
            BsnEntry::CachedScene(BsnScene::Asset(path))
        } else if input.peek::<At>() {
            let _ = input.parse::<At>()?;
            BsnEntry::UncachedScene(input.parse::<BsnScene>()?)
        } else {
            let path = input.parse::<Path>()?;
            if input.peek::<LBracket>() {
                let scene_list = input.parse::<BsnSceneList>()?;
                BsnEntry::RelatedSceneList {
                    relationship_target_path: path,
                    scene_list,
                }
            } else {
                BsnEntry::FromTemplatePatch(BsnType {
                    path,
                    variant: None,
                    fields: input.parse::<BsnFields>()?,
                })
            }
        })
    }
}

impl Parse for BsnScene {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        Ok(if input.peek::<StringLit>() {
            BsnScene::Asset(input.parse::<StringLit>()?)
        } else {
            BsnScene::SceneComponent(input.parse::<BsnType>()?)
        })
    }
}

impl Parse for BsnType {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let path = input.parse::<Path>()?;
        let fields = input.parse::<BsnFields>()?;
        Ok(BsnType {
            path,
            variant: None,
            fields,
        })
    }
}

impl Parse for BsnFields {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        Ok(if input.peek::<LBrace>() {
            let _ = input.parse::<LBrace>()?;
            let mut fields = Vec::new();
            while !input.is_empty_or_closing_delimiter() {
                fields.push(input.parse::<BsnNamedField>()?);
                if input.is_empty_or_closing_delimiter() {
                    break;
                }
                input.parse::<Comma>()?;
            }
            let _ = input.parse::<RBrace>()?;
            BsnFields::NamedFields(fields)
        } else if input.peek::<LParen>() {
            let _ = input.parse::<LParen>()?;
            let fields = parse_values(input)?;
            let _ = input.parse::<RParen>()?;
            BsnFields::UnnamedFields(fields)
        } else {
            BsnFields::Unit
        })
    }
}

/// Comma-separated values up to a closing delimiter, a trailing comma allowed.
fn parse_values(input: &mut ParseStream<'_>) -> Result<Vec<BsnValue>, ParseError> {
    let mut values = Vec::new();
    while !input.is_empty_or_closing_delimiter() {
        values.push(input.parse::<BsnValue>()?);
        if input.is_empty_or_closing_delimiter() {
            break;
        }
        input.parse::<Comma>()?;
    }
    Ok(values)
}

impl Parse for BsnNamedField {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let name = input.parse::<Ident>()?;
        let _ = input.parse::<Colon>()?;
        let value = input.parse::<BsnValue>()?;
        Ok(BsnNamedField {
            name: name.0,
            value,
        })
    }
}

impl Parse for BsnValue {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        Ok(if input.peek::<Float>() {
            let value = input.parse::<Float>()?;
            BsnValue::Float(value.0)
        } else if input.peek::<Int>() {
            let value = input.parse::<Int>()?;
            BsnValue::Int(value.0)
        } else if input.peek::<StringLit>() {
            let value = input.parse::<StringLit>()?;
            BsnValue::String(value.0)
        } else if input.peek::<Bool>() {
            let value = input.parse::<Bool>()?;
            BsnValue::Bool(value.0)
        } else if input.peek::<LBracket>() {
            let _ = input.parse::<LBracket>()?;
            let items = parse_values(input)?;
            let _ = input.parse::<RBracket>()?;
            BsnValue::List(items)
        } else if is_map_literal(input) {
            let _ = input.parse::<Ident>()?;
            BsnValue::Map(parse_map_entries(input)?)
        } else if input.peek::<At>() {
            // Read for compatibility with the dynamic-BSN grammar, which
            // allowed `@` before a value's type; it means nothing there.
            let _ = input.parse::<At>()?;
            BsnValue::Type(input.parse::<BsnType>()?)
        } else if input.peek::<Ident>() {
            let value = input.parse::<BsnType>()?;
            BsnValue::Type(value)
        } else {
            return Err(input.error("Expected a value type"));
        })
    }
}

/// `map` is a keyword only right before `[`; anywhere else it is a name.
fn is_map_literal(input: &mut ParseStream<'_>) -> bool {
    matches!(input.peek_token().map(|t| &t.token), Some(Token::Ident(word)) if word == "map")
        && matches!(
            input.peek_nth_token(1).map(|t| &t.token),
            Some(Token::LBracket)
        )
}

/// `[(key, value), ...]`, after the `map` keyword.
fn parse_map_entries(input: &mut ParseStream<'_>) -> Result<Vec<(BsnValue, BsnValue)>, ParseError> {
    let _ = input.parse::<LBracket>()?;
    let mut entries = Vec::new();
    while !input.is_empty_or_closing_delimiter() {
        let _ = input.parse::<LParen>()?;
        let key = input.parse::<BsnValue>()?;
        let _ = input.parse::<Comma>()?;
        let value = input.parse::<BsnValue>()?;
        let _ = input.parse::<RParen>()?;
        entries.push((key, value));
        if input.is_empty_or_closing_delimiter() {
            break;
        }
        input.parse::<Comma>()?;
    }
    let _ = input.parse::<RBracket>()?;
    Ok(entries)
}

impl Parse for BsnSceneList {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let items = input.bracketed::<BsnSceneListItems>()?;
        Ok(BsnSceneList(items))
    }
}

impl Parse for BsnSceneListItems {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let mut scenes = Vec::new();
        while !input.is_empty_or_closing_delimiter() {
            scenes.push(input.parse::<Bsn>()?);
            if input.is_empty_or_closing_delimiter() {
                break;
            }
            input.parse::<DoubleMinus>()?;
        }

        Ok(BsnSceneListItems(scenes))
    }
}

impl Parse for Path {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let leading_colon = if input.peek::<DoubleColon>() {
            let _ = input.parse::<DoubleColon>()?;
            true
        } else {
            false
        };
        let mut segments = Vec::new();
        segments.push(input.parse::<PathSegment>()?);
        while input.peek::<DoubleColon>() {
            let _ = input.parse::<DoubleColon>()?;
            segments.push(input.parse::<PathSegment>()?);
        }
        Ok(Path {
            leading_colon,
            segments,
        })
    }
}

impl Parse for PathSegment {
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
        let ident = input.parse::<Ident>()?;
        let mut arguments = Vec::new();
        if input.peek::<Less>() {
            let _ = input.parse::<Less>()?;
            loop {
                arguments.push(input.parse::<Path>()?);
                if input.peek::<Greater>() {
                    break;
                }
                input.parse::<Comma>()?;
            }
            let _ = input.parse::<Greater>()?;
        }
        Ok(PathSegment { ident, arguments })
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::{
        parse_stream::ParseStream,
        span::Span,
        types::{
            Bsn, BsnEntry, BsnFields, BsnListRoot, BsnNamedField, BsnRoot, BsnScene, BsnSceneList,
            BsnSceneListItems, BsnType, BsnValue, Ident, Path, StringLit,
        },
    };

    fn parse_root(text: &str) -> BsnRoot {
        ParseStream::from(Span::from(text))
            .parse::<BsnRoot>()
            .unwrap()
    }

    fn parse_list(text: &str) -> BsnListRoot {
        ParseStream::from(Span::from(text))
            .parse::<BsnListRoot>()
            .unwrap()
    }

    #[test]
    fn parse_bsn() {
        let bsn_root = parse_root(
            r#"
#A
:"inherited.bsn"
bevy_transform::Transform { x: 1.0 }
Children [
    bevy_transform::Transform
    Foo { nested: Bar { value: 1 } }
    Marker
    Score(0, 1)
    --
    #B
    Marker
    Children [
        #C
    ]
]
"#,
        );
        assert_eq!(
            bsn_root,
            BsnRoot(Bsn {
                entries: vec![
                    BsnEntry::Name("A".into()),
                    BsnEntry::CachedScene(BsnScene::Asset(StringLit("inherited.bsn".into()))),
                    BsnEntry::FromTemplatePatch(BsnType::new(
                        ["bevy_transform", "Transform"],
                        BsnFields::NamedFields(vec![BsnNamedField::new("x", BsnValue::Float(1.0))])
                    )),
                    BsnEntry::RelatedSceneList {
                        relationship_target_path: Path::from(["Children"]),
                        scene_list: BsnSceneList(BsnSceneListItems(vec![
                            Bsn {
                                entries: vec![
                                    BsnEntry::FromTemplatePatch(BsnType::path([
                                        "bevy_transform",
                                        "Transform"
                                    ])),
                                    BsnEntry::FromTemplatePatch(BsnType::new(
                                        ["Foo"],
                                        BsnFields::NamedFields(vec![BsnNamedField::new(
                                            "nested",
                                            BsnValue::Type(BsnType::new(
                                                ["Bar"],
                                                BsnFields::NamedFields(vec![BsnNamedField::new(
                                                    "value",
                                                    BsnValue::Int(1)
                                                )])
                                            ))
                                        )])
                                    )),
                                    BsnEntry::FromTemplatePatch(BsnType::path(["Marker"])),
                                    BsnEntry::FromTemplatePatch(BsnType::new(
                                        ["Score"],
                                        BsnFields::UnnamedFields(vec![
                                            BsnValue::Int(0),
                                            BsnValue::Int(1)
                                        ])
                                    ))
                                ]
                            },
                            Bsn {
                                entries: vec![
                                    BsnEntry::Name("B".into()),
                                    BsnEntry::FromTemplatePatch(BsnType::path(["Marker"])),
                                    BsnEntry::RelatedSceneList {
                                        relationship_target_path: Path::from(["Children"]),
                                        scene_list: BsnSceneList(BsnSceneListItems(vec![Bsn {
                                            entries: vec![BsnEntry::Name("C".into())]
                                        }]))
                                    }
                                ]
                            }
                        ]))
                    }
                ]
            })
        );
    }

    #[test]
    fn a_comma_does_not_separate_entities() {
        let mut input = ParseStream::from(Span::from("Children [ A, B ]"));
        assert!(input.parse::<BsnRoot>().is_err());
    }

    #[test]
    fn a_list_root_holds_entities_separated_by_double_minus() {
        let root = parse_list("#A Marker -- #B --");
        assert_eq!(root.0.0.len(), 2);
        assert_eq!(root.0.0[1].entries, vec![BsnEntry::Name("B".into())]);
    }

    #[test]
    fn a_list_root_rejects_a_stray_closing_delimiter() {
        let mut input = ParseStream::from(Span::from("A ]"));
        assert!(input.parse::<BsnListRoot>().is_err());
    }

    #[test]
    fn a_quoted_name_reads_as_a_name() {
        let root = parse_root(r#"#"Main Camera""#);
        assert_eq!(
            root.0.entries,
            vec![BsnEntry::Name(Ident("Main Camera".into()))]
        );
    }

    #[test]
    fn an_at_sign_includes_an_uncached_scene() {
        let root = parse_root(r#"@"base.bsn" @Builder { n: 1 }"#);
        assert_eq!(
            root.0.entries,
            vec![
                BsnEntry::UncachedScene(BsnScene::Asset(StringLit("base.bsn".into()))),
                BsnEntry::UncachedScene(BsnScene::SceneComponent(BsnType::new(
                    ["Builder"],
                    BsnFields::NamedFields(vec![BsnNamedField::new("n", BsnValue::Int(1))])
                ))),
            ]
        );
    }

    #[test]
    fn generic_arguments_stay_on_their_segment() {
        let root = parse_root("a::Handle<b::Image> Wrap<Vec<f32>, u8>");
        let paths: Vec<String> = root
            .0
            .entries
            .iter()
            .map(|entry| match entry {
                BsnEntry::FromTemplatePatch(t) => t.path.to_type_path(),
                other => panic!("expected a patch, got {other:?}"),
            })
            .collect();
        assert_eq!(paths, ["a::Handle<b::Image>", "Wrap<Vec<f32>, u8>"]);
    }

    #[test]
    fn lists_and_maps_read_as_values() {
        let root = parse_root(r#"C { l: [1, -2.5,], m: map[("a", [true])], map: 3 }"#);
        let BsnEntry::FromTemplatePatch(BsnType {
            fields: BsnFields::NamedFields(fields),
            ..
        }) = &root.0.entries[0]
        else {
            panic!("expected a patch with named fields");
        };
        assert_eq!(
            fields[0].value,
            BsnValue::List(vec![BsnValue::Int(1), BsnValue::Float(-2.5)])
        );
        assert_eq!(
            fields[1].value,
            BsnValue::Map(vec![(
                BsnValue::String("a".into()),
                BsnValue::List(vec![BsnValue::Bool(true)])
            )])
        );
        assert_eq!(fields[2].name, "map");
    }
}
