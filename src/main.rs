#[tokio::main]
async fn main() {
    // Test stacks sweeper board generation
    let board = stacks_wars_be::games::stacks_sweeper::Board::generate(6, 0.5);
    println!("{:#?}", board);

    stacks_wars_be::start_server().await;
}
