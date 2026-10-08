// Quotes about building great things, one shown at random above the new-task composer.
// Only quotes with a well-documented author; widely misattributed lines are left out.

export const QUOTES = [
  { text: 'The best way to predict the future is to invent it.', author: 'Alan Kay' },
  { text: 'Real artists ship.', author: 'Steve Jobs' },
  { text: 'Simplicity is prerequisite for reliability.', author: 'Edsger W. Dijkstra' },
  { text: 'Make it work, make it right, make it fast.', author: 'Kent Beck' },
  { text: 'Talk is cheap. Show me the code.', author: 'Linus Torvalds' },
  { text: 'Perfection is achieved, not when there is nothing more to add, but when there is nothing left to take away.', author: 'Antoine de Saint-Exupéry' },
  { text: 'Design is not just what it looks like and feels like. Design is how it works.', author: 'Steve Jobs' },
  { text: 'If you are not embarrassed by the first version of your product, you’ve launched too late.', author: 'Reid Hoffman' },
  { text: 'Make something people want.', author: 'Paul Graham' },
  { text: 'Programs must be written for people to read, and only incidentally for machines to execute.', author: 'Harold Abelson' },
  { text: 'Premature optimization is the root of all evil.', author: 'Donald Knuth' },
  { text: 'The details are not the details. They make the design.', author: 'Charles Eames' },
  { text: 'Good design is as little design as possible.', author: 'Dieter Rams' },
  { text: 'The only way to do great work is to love what you do.', author: 'Steve Jobs' },
  { text: 'Any sufficiently advanced technology is indistinguishable from magic.', author: 'Arthur C. Clarke' },
  { text: 'The most dangerous phrase in the language is “We’ve always done it this way.”', author: 'Grace Hopper' },
  { text: 'Walking on water and developing software from a specification are easy if both are frozen.', author: 'Edward V. Berard' },
  { text: 'Simple things should be simple, complex things should be possible.', author: 'Alan Kay' },
  { text: 'The function of good software is to make the complex appear to be simple.', author: 'Grady Booch' },
  { text: 'Controlling complexity is the essence of computer programming.', author: 'Brian Kernighan' },
  { text: 'The people who are crazy enough to think they can change the world are the ones who do.', author: 'Apple, “Think Different”' },
  { text: 'Release early. Release often. And listen to your customers.', author: 'Eric S. Raymond' },
  { text: 'The best code is no code at all.', author: 'Jeff Atwood' },
  { text: 'Code is like humor. When you have to explain it, it’s bad.', author: 'Cory House' },
  { text: 'Software is eating the world.', author: 'Marc Andreessen' },
  { text: 'A ship in harbor is safe, but that is not what ships are built for.', author: 'John A. Shedd' },
  { text: 'Fall seven times, stand up eight.', author: 'Japanese proverb' },
  { text: 'Great things are not done by impulse, but by a series of small things brought together.', author: 'Vincent van Gogh' },
  { text: 'Less is more.', author: 'Ludwig Mies van der Rohe' },
  { text: 'Form ever follows function.', author: 'Louis Sullivan' },
  { text: 'Your most unhappy customers are your greatest source of learning.', author: 'Bill Gates' },
  { text: 'Make each program do one thing well.', author: 'Doug McIlroy' },
  { text: 'Testing shows the presence, not the absence of bugs.', author: 'Edsger W. Dijkstra' },
  { text: 'Optimism is an occupational hazard of programming: feedback is the treatment.', author: 'Kent Beck' },
  { text: 'The impediment to action advances action. What stands in the way becomes the way.', author: 'Marcus Aurelius' },
  { text: 'Well done is better than well said.', author: 'Benjamin Franklin' },
  { text: 'Do. Or do not. There is no try.', author: 'Yoda' },
  { text: 'If it hurts, do it more often.', author: 'Martin Fowler' },
  { text: 'Any fool can write code that a computer can understand. Good programmers write code that humans can understand.', author: 'Martin Fowler' },
  { text: 'Everyone knows that debugging is twice as hard as writing a program in the first place.', author: 'Brian Kernighan' },
];

/** A random quote, avoiding the one shown last in this browser tab. */
let last = -1;
export function randomQuote() {
  let index = Math.floor(Math.random() * QUOTES.length);
  if (index === last) index = (index + 1) % QUOTES.length;
  last = index;
  return QUOTES[index];
}
