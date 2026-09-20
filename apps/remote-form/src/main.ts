import { renderApp } from './app';
import './styles.css';

const container = document.getElementById('root');

if (!container) {
  throw new Error('Root container #root was not found in index.html');
}

renderApp(container);
