const GAME_ID_FORMAT: u128 = lexical::NumberFormatBuilder::from_radix(36);

pub fn parse_full_game_id_str(src: &str) -> Result<(u8, u32), lexical::Error> {
    let result: u64 = lexical::parse_with_options::<_, _, GAME_ID_FORMAT>(
        &src,
        &lexical::parse_integer_options::Options::default(),
    )?;

    Ok(((result & (u8::MAX as u64)) as u8, (result >> 8) as u32))
}

pub fn to_full_game_id_str(server_id: u8, game_id: u32) -> String {
    lexical::to_string_with_options::<_, GAME_ID_FORMAT>(
        (u64::from(game_id) << 8) + u64::from(server_id),
        &lexical::write_integer_options::Options::default(),
    )
}
